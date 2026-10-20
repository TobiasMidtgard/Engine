//! The GPU side of virtual texturing, owned by the mesh feature: the cache textures, the page
//! tables and texture records (one storage buffer), the feedback hash table and its readback,
//! the tile loader threads, and the per-frame residency work. See the module docs of
//! [`crate::virtual_texture`].

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use crossbeam_channel::{Receiver, Sender};
use engine_assets::{Assets, Handle};

use super::provider::{ImageProvider, TILE_BYTES, TileProvider, tile_levels};
use super::residency::{
    BORDER, Layout, MAX_LEVELS, SLOT, SlotOwner, SlotPool, TILE, TextureResidency, Tile,
};
use super::{VirtualTextureStats, VirtualTextures};
use crate::features::GpuShared;
use crate::settings::RenderSettings;
use crate::texture::{Image, PixelFormat, f32_to_f16};

/// Words before the texture records: frame, mip bias, cache size (texels, f32), slots per row,
/// cache levels, unused.
pub const HEADER_WORDS: u32 = 8;
/// Words per texture record: width, height, root | atlas << 8, unused, 16 level offsets,
/// mean color (4 × f16), unused, then the texture at 4×4 texels (16 × 4 f16, rows from the
/// top): the levels past the root slot's mip tail, filtered in the shader.
pub const INFO_WORDS: u32 = 56;
/// First word of the 4×4 level in a record.
const LOW_WORD: usize = 24;
/// Most virtual textures drawn at once (ids are 8 bits, 0 = not virtual).
pub const MAX_TEXTURES: usize = 255;
/// First word of the page tables.
const PAGE_BASE: u32 = HEADER_WORDS + INFO_WORDS * MAX_TEXTURES as u32;
/// Feedback hash table entries (a power of two).
pub const FEEDBACK_ENTRIES: u32 = 16384;
const READBACKS: usize = 3;
/// Virtual textures nobody drew for this many frames leave the cache.
const EVICT_AFTER_FRAMES: u64 = 600;
/// Cache textures: linear data, sRGB color.
const ATLAS_LINEAR: usize = 0;
const ATLAS_SRGB: usize = 1;

const FREE: u8 = 0;
const IN_FLIGHT: u8 = 1;
const MAPPING: u8 = 2;
const MAPPED: u8 = 3;
const FAILED: u8 = 4;

struct Readback {
    buffer: wgpu::Buffer,
    state: Arc<AtomicU8>,
}

/// What a virtual texture is made from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum SourceKey {
    /// Registered with [`VirtualTextures`] (image index, generation).
    Registered(u32, u64),
    /// An image over the size threshold (image index, asset version).
    Image(u32, u64),
}

struct VirtualTex {
    key: (SourceKey, bool),
    provider: Arc<dyn TileProvider>,
    residency: TextureResidency,
    atlas: usize,
    /// First word of its page table in the data buffer.
    page_offset: u32,
    last_used: u64,
    /// Counts up when the id is reused, so stale loads are dropped.
    generation: u32,
    mean: [f32; 4],
    /// The texture at 4×4 texels (decoded like `mean`).
    low: [[f32; 4]; 16],
    /// The record (size, levels, mean) must be uploaded.
    info_dirty: bool,
    /// The root tile is being loaded.
    root_requested: bool,
    failed: bool,
}

/// One cache texture.
struct Atlas {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    /// Created at full size (else a 1-slot stand-in until a texture needs it).
    real: bool,
}

/// A tile to load on a worker.
struct Job {
    id: u8,
    generation: u32,
    tile: Tile,
    provider: Arc<dyn TileProvider>,
    srgb: bool,
    levels: u32,
    compress: bool,
    /// The texture's root tile: also measure its mean color.
    root: bool,
}

/// A loaded tile: its cache levels, ready to upload.
struct Loaded {
    id: u8,
    generation: u32,
    tile: Tile,
    data: Result<Vec<u8>, String>,
    /// Mean color and 4×4 level of a root tile (linear for sRGB color).
    mean: Option<([f32; 4], [[f32; 4]; 16])>,
    /// Frame it arrived (stale ones are dropped).
    arrived: u64,
}

struct Workers {
    jobs: Sender<Job>,
    results: Receiver<Loaded>,
}

impl Workers {
    fn spawn() -> Self {
        let (jobs, job_rx) = crossbeam_channel::unbounded::<Job>();
        let (result_tx, results) = crossbeam_channel::unbounded::<Loaded>();
        let threads = std::thread::available_parallelism()
            .map_or(2, |n| n.get())
            .saturating_sub(1)
            .clamp(1, 3);
        for i in 0..threads {
            let rx = job_rx.clone();
            let tx = result_tx.clone();
            let _ = std::thread::Builder::new()
                .name(format!("virtual texture loader {i}"))
                .spawn(move || {
                    let mut tile = vec![0u8; TILE_BYTES];
                    while let Ok(job) = rx.recv() {
                        let mut mean = None;
                        let data = job
                            .provider
                            .read_tile(job.tile.level, job.tile.x, job.tile.y, &mut tile)
                            .map(|()| {
                                if job.root {
                                    let (w, h) = job.provider.size();
                                    let size = (
                                        (w >> job.tile.level).clamp(1, TILE),
                                        (h >> job.tile.level).clamp(1, TILE),
                                    );
                                    mean = Some(root_summary(&tile, job.srgb, size));
                                }
                                let levels = tile_levels(&tile, job.srgb, job.levels);
                                if job.compress {
                                    super::bc7::compress_levels(&levels, job.levels)
                                } else {
                                    levels
                                }
                            });
                        let loaded = Loaded {
                            id: job.id,
                            generation: job.generation,
                            tile: job.tile,
                            data,
                            mean,
                            arrived: 0,
                        };
                        if tx.send(loaded).is_err() {
                            break;
                        }
                    }
                });
        }
        Self { jobs, results }
    }
}

/// The virtual texture cache, page tables and streaming of one renderer.
pub(crate) struct VirtualTextureSystem {
    feedback_writes: bool,
    bc_supported: bool,
    compressed: bool,
    /// Mip levels of each slot in the cache textures.
    levels: u32,
    capacity: u32,
    per_row: u32,
    atlases: [Atlas; 2],
    pools: [SlotPool; 2],
    textures: Vec<Option<VirtualTex>>,
    by_key: HashMap<(SourceKey, bool), u8>,
    /// Image copies made for images over the threshold, by image index.
    snapshots: HashMap<(u32, bool), (u64, Arc<ImageProvider>)>,
    /// Images over the threshold that can't be virtual (by index, version): drawn whole.
    rejected: HashMap<u32, u64>,
    data: wgpu::Buffer,
    /// Words the page tables need (the buffer grows to it at the start of a frame).
    data_words: u32,
    /// Words the buffer holds.
    data_capacity: u32,
    /// Page table ranges in use (start word, words), sorted.
    page_ranges: Vec<(u32, u32)>,
    feedback: wgpu::Buffer,
    readbacks: Vec<Readback>,
    written: Option<usize>,
    sampler: wgpu::Sampler,
    workers: Option<Workers>,
    in_flight: HashSet<(u8, u32, Tile)>,
    ready: Vec<Loaded>,
    /// Bumped when a resource the texture sets bind is recreated.
    bind_generation: u64,
    frame: u64,
    settings_key: (u32, bool),
    /// Generations handed to textures (never reused, so stale loads are recognized).
    next_generation: u32,
    upload_budget: u32,
    mip_bias: f32,
    stats: VirtualTextureStats,
    /// Scratch.
    wanted: HashMap<(u8, Tile), u32>,
    wanted_sorted: Vec<((u8, Tile), u32)>,
}

fn stand_in_atlas(device: &wgpu::Device, srgb: bool) -> Atlas {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("virtual texture cache (empty)"),
        size: wgpu::Extent3d {
            width: 4,
            height: 4,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: if srgb {
            wgpu::TextureFormat::Rgba8UnormSrgb
        } else {
            wgpu::TextureFormat::Rgba8Unorm
        },
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let view = texture.create_view(&Default::default());
    Atlas {
        texture,
        view,
        real: false,
    }
}

impl VirtualTextureSystem {
    pub fn new(gpu: &GpuShared) -> Self {
        let device = &gpu.device;
        let feedback_writes = feedback_writes(gpu);
        let bc_supported = gpu
            .features
            .contains(wgpu::Features::TEXTURE_COMPRESSION_BC);
        let data_words = PAGE_BASE + 4096;
        let data = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("virtual texture page tables"),
            size: data_words as u64 * 4,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let feedback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("virtual texture feedback"),
            size: FEEDBACK_ENTRIES as u64 * 4,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_SRC
                | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let readbacks = (0..READBACKS)
            .map(|_| Readback {
                buffer: device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("virtual texture feedback readback"),
                    size: FEEDBACK_ENTRIES as u64 * 4,
                    usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }),
                state: Arc::new(AtomicU8::new(FREE)),
            })
            .collect();
        // Trilinear over a slot's own levels (the tile, then coarser copies of it; the root's
        // slot is the mip tail), 8x anisotropic: the border covers the footprint. The hardware
        // picks the slot level from the gradients, so where it filters isotropically (some
        // software drivers) it goes coarser within the slot, like an ordinary texture would.
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("virtual texture sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            anisotropy_clamp: 8,
            ..Default::default()
        });
        Self {
            feedback_writes,
            bc_supported,
            compressed: false,
            levels: 4,
            capacity: 0,
            per_row: 1,
            atlases: [stand_in_atlas(device, false), stand_in_atlas(device, true)],
            pools: [SlotPool::new(0), SlotPool::new(0)],
            textures: (0..MAX_TEXTURES).map(|_| None).collect(),
            by_key: HashMap::new(),
            snapshots: HashMap::new(),
            rejected: HashMap::new(),
            data,
            data_words,
            data_capacity: data_words,
            page_ranges: Vec::new(),
            feedback,
            readbacks,
            written: None,
            sampler,
            workers: None,
            in_flight: HashSet::new(),
            ready: Vec::new(),
            bind_generation: 1,
            frame: 0,
            settings_key: (0, false),
            next_generation: 0,
            upload_budget: 32,
            mip_bias: 0.0,
            stats: VirtualTextureStats::default(),
            wanted: HashMap::new(),
            wanted_sorted: Vec::new(),
        }
    }

    /// Changes when the bindings of [`VirtualTextureSystem::bind_entries`] change: texture sets
    /// must then be rebuilt.
    pub fn bind_generation(&self) -> u64 {
        self.bind_generation
    }

    /// Layout entries of the virtual texture bindings, from `first` (group 2 of the mesh
    /// feature): page tables, linear cache, sRGB cache, sampler, feedback.
    pub fn layout_entries(first: u32, feedback_writes: bool) -> [wgpu::BindGroupLayoutEntry; 5] {
        let texture = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let storage = |binding, read_only| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let sampler = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        };
        [
            storage(first, true),
            texture(first + 1),
            texture(first + 2),
            sampler(first + 3),
            storage(first + 4, !feedback_writes),
        ]
    }

    /// Bind group entries matching [`VirtualTextureSystem::layout_entries`].
    pub fn bind_entries(&self, first: u32) -> [wgpu::BindGroupEntry<'_>; 5] {
        [
            wgpu::BindGroupEntry {
                binding: first,
                resource: self.data.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: first + 1,
                resource: wgpu::BindingResource::TextureView(&self.atlases[ATLAS_LINEAR].view),
            },
            wgpu::BindGroupEntry {
                binding: first + 2,
                resource: wgpu::BindingResource::TextureView(&self.atlases[ATLAS_SRGB].view),
            },
            wgpu::BindGroupEntry {
                binding: first + 3,
                resource: wgpu::BindingResource::Sampler(&self.sampler),
            },
            wgpu::BindGroupEntry {
                binding: first + 4,
                resource: self.feedback.as_entire_binding(),
            },
        ]
    }

    /// Counters for `VirtualTextureStats`.
    pub fn stats(&self) -> &VirtualTextureStats {
        &self.stats
    }

    /// Bytes of one slot with its mips in the cache.
    fn slot_bytes(&self) -> u64 {
        let mut bytes = 0u64;
        for l in 0..self.levels {
            let s = (SLOT >> l) as u64;
            bytes += if self.compressed { s * s } else { s * s * 4 };
        }
        bytes
    }

    /// Drop every tile and texture (the cache is recreated at a new size or format).
    fn reset(&mut self, gpu: &GpuShared) {
        for t in self.textures.iter_mut().flatten() {
            t.residency = TextureResidency::new(t.residency.layout.clone());
            t.root_requested = false;
            self.next_generation = self.next_generation.wrapping_add(1);
            t.generation = self.next_generation;
            t.info_dirty = true;
        }
        self.in_flight.clear();
        self.ready.clear();
        self.atlases = [
            stand_in_atlas(&gpu.device, false),
            stand_in_atlas(&gpu.device, true),
        ];
        self.pools = [SlotPool::new(self.capacity), SlotPool::new(self.capacity)];
        self.bind_generation += 1;
    }

    /// Create cache texture `atlas` at full size.
    fn ensure_atlas(&mut self, gpu: &GpuShared, atlas: usize) {
        if self.atlases[atlas].real {
            return;
        }
        let size = self.per_row * SLOT;
        let srgb = atlas == ATLAS_SRGB;
        let format = match (self.compressed, srgb) {
            (true, true) => wgpu::TextureFormat::Bc7RgbaUnormSrgb,
            (true, false) => wgpu::TextureFormat::Bc7RgbaUnorm,
            (false, true) => wgpu::TextureFormat::Rgba8UnormSrgb,
            (false, false) => wgpu::TextureFormat::Rgba8Unorm,
        };
        let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some(if srgb {
                "virtual texture cache (sRGB)"
            } else {
                "virtual texture cache (linear)"
            }),
            size: wgpu::Extent3d {
                width: size,
                height: size,
                depth_or_array_layers: 1,
            },
            mip_level_count: self.levels,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        self.atlases[atlas] = Atlas {
            texture,
            view,
            real: true,
        };
        self.bind_generation += 1;
    }

    /// Start of the mesh feature's prepare: apply settings, read feedback, collect loaded
    /// tiles, start new loads and upload up to the frame's budget.
    pub fn begin_frame(&mut self, gpu: &GpuShared, settings: &RenderSettings) {
        self.frame += 1;
        self.stats.tiles_loaded = 0;
        self.upload_budget = settings.virtual_texture_upload_budget;
        self.mip_bias = settings.virtual_texture_mip_bias;
        let max_dim = gpu.device.limits().max_texture_dimension_2d;
        let per_row = ((settings.virtual_texture_cache_tiles.max(4) as f64)
            .sqrt()
            .ceil() as u32)
            .clamp(2, (max_dim / SLOT).max(2))
            .min(256);
        let compressed = settings.virtual_texture_compression && self.bc_supported;
        if (per_row, compressed) != self.settings_key {
            self.settings_key = (per_row, compressed);
            self.per_row = per_row;
            self.capacity = (per_row * per_row).min(u16::MAX as u32);
            self.compressed = compressed;
            self.levels = if compressed { 3 } else { 4 };
            self.reset(gpu);
        }

        // Tiles loaded on the workers.
        if let Some(w) = &self.workers {
            while let Ok(mut l) = w.results.try_recv() {
                self.in_flight.remove(&(l.id, l.generation, l.tile));
                l.arrived = self.frame;
                self.ready.push(l);
            }
        }
        self.read_feedback(gpu);
        if !self.feedback_writes {
            self.request_coarsest();
        }
        self.dispatch();
        self.commit(gpu);

        if self.frame.is_multiple_of(120) {
            self.evict_unused();
        }
        self.grow_data(gpu);
        self.upload_tables(gpu);
        self.update_stats();
    }

    /// Map last frame's feedback copy and turn finished ones into requests.
    fn read_feedback(&mut self, gpu: &GpuShared) {
        if let Some(slot) = self.written.take() {
            let rb = &self.readbacks[slot];
            rb.state.store(MAPPING, Ordering::Release);
            let state = rb.state.clone();
            rb.buffer
                .slice(..)
                .map_async(wgpu::MapMode::Read, move |r| {
                    state.store(if r.is_ok() { MAPPED } else { FAILED }, Ordering::Release);
                });
        }
        let _ = gpu.device.poll(wgpu::PollType::Poll);
        for i in 0..self.readbacks.len() {
            match self.readbacks[i].state.load(Ordering::Acquire) {
                MAPPED => {
                    let keys: Vec<u32> = {
                        let rb = &self.readbacks[i];
                        match rb.buffer.slice(..).get_mapped_range() {
                            Ok(view) => bytemuck::cast_slice::<u8, u32>(&view)
                                .iter()
                                .copied()
                                .filter(|&k| k != 0)
                                .collect(),
                            Err(_) => Vec::new(),
                        }
                    };
                    self.readbacks[i].buffer.unmap();
                    self.readbacks[i].state.store(FREE, Ordering::Release);
                    self.process_requests(&keys);
                }
                FAILED => {
                    self.readbacks[i].buffer.unmap();
                    self.readbacks[i].state.store(FREE, Ordering::Release);
                }
                _ => {}
            }
        }
    }

    /// Feedback keys (texture id, level, x, y) → touch resident tiles, want missing ones.
    fn process_requests(&mut self, keys: &[u32]) {
        let frame = self.frame;
        let mut requests = 0;
        let mut misses = 0;
        let mut seen = HashSet::with_capacity(keys.len());
        for &key in keys {
            if !seen.insert(key) {
                continue;
            }
            let id = (key & 0xff) as u8;
            let tile = Tile::new((key >> 8) & 15, (key >> 12) & 1023, (key >> 22) & 1023);
            let Some(Some(tex)) = self.textures.get(id as usize - 1) else {
                continue;
            };
            if !tex.residency.layout.contains(tile.level, tile.x, tile.y) || tex.failed {
                continue;
            }
            requests += 1;
            if !tex.residency.resident(tile) {
                misses += 1;
            }
            // Walk up to the root: touch what is resident, want what isn't.
            let atlas = tex.atlas;
            let root = tex.residency.layout.root;
            let mut t = tile;
            loop {
                if let Some(slot) = tex.residency.slot(t) {
                    self.pools[atlas].touch(slot, frame);
                } else {
                    *self.wanted.entry((id, t)).or_insert(0) += 1;
                }
                if t.level >= root {
                    break;
                }
                t = t.parent();
            }
        }
        self.stats.feedback_requests = requests;
        self.stats.feedback_misses = misses;
    }

    /// Without feedback: want every tile of the coarsest levels that fit half the cache.
    fn request_coarsest(&mut self) {
        let active: Vec<u8> = (0..MAX_TEXTURES)
            .filter(|&i| {
                self.textures[i]
                    .as_ref()
                    .is_some_and(|t| t.residency.ready() && t.last_used + 2 >= self.frame)
            })
            .map(|i| i as u8 + 1)
            .collect();
        if active.is_empty() {
            return;
        }
        let share = self.capacity / 2 / active.len() as u32;
        for id in active {
            let Some(tex) = &self.textures[id as usize - 1] else {
                continue;
            };
            let layout = &tex.residency.layout;
            let mut total = 0;
            for level in (0..=layout.root).rev() {
                let (px, py) = layout.pages(level);
                total += px * py;
                if total > share {
                    break;
                }
                for y in 0..py {
                    for x in 0..px {
                        let t = Tile::new(level, x, y);
                        if let Some(slot) = tex.residency.slot(t) {
                            self.pools[tex.atlas].touch(slot, self.frame);
                        } else {
                            self.wanted.insert((id, t), 1);
                        }
                    }
                }
            }
        }
    }

    /// Start loading wanted tiles: coarse levels first, then the most asked for.
    fn dispatch(&mut self) {
        if self.wanted.is_empty() {
            return;
        }
        let cap = (self.upload_budget.max(1) as usize * 4).max(16);
        self.wanted_sorted.clear();
        self.wanted_sorted.extend(self.wanted.drain());
        self.wanted_sorted
            .sort_unstable_by(|a, b| b.0.1.level.cmp(&a.0.1.level).then(b.1.cmp(&a.1)));
        let workers = self.workers.get_or_insert_with(Workers::spawn);
        for &((id, tile), _) in &self.wanted_sorted {
            if self.in_flight.len() + self.ready.len() >= cap {
                break;
            }
            let Some(tex) = &self.textures[id as usize - 1] else {
                continue;
            };
            let key = (id, tex.generation, tile);
            if self.in_flight.contains(&key)
                || self
                    .ready
                    .iter()
                    .any(|l| (l.id, l.generation, l.tile) == key)
            {
                continue;
            }
            let job = Job {
                id,
                generation: tex.generation,
                tile,
                provider: tex.provider.clone(),
                srgb: tex.provider.srgb() && tex.atlas == ATLAS_SRGB,
                levels: self.levels,
                compress: self.compressed,
                root: tile.level == tex.residency.layout.root,
            };
            if workers.jobs.send(job).is_ok() {
                self.in_flight.insert(key);
            }
        }
        self.wanted_sorted.clear();
    }

    /// Load a texture's root tile (it must be resident before the texture is drawn).
    fn request_root(&mut self, id: u8) {
        let Some(tex) = &mut self.textures[id as usize - 1] else {
            return;
        };
        if tex.root_requested || tex.residency.ready() {
            return;
        }
        tex.root_requested = true;
        let root = tex.residency.root();
        self.wanted.insert((id, root), u32::MAX);
    }

    /// Upload loaded tiles (coarse first, at most the budget; roots always), evicting the
    /// least recently used leaves when the cache is full.
    fn commit(&mut self, gpu: &GpuShared) {
        if self.ready.is_empty() {
            return;
        }
        let frame = self.frame;
        self.ready
            .sort_unstable_by_key(|r| std::cmp::Reverse(r.tile.level));
        let mut budget = self.upload_budget;
        let mut keep = Vec::new();
        let ready = std::mem::take(&mut self.ready);
        for l in ready {
            // What the texture says about this tile (copied out so `self` is free below).
            let Some((atlas, is_root, can_commit, parent_slot)) =
                self.textures[l.id as usize - 1].as_ref().and_then(|tex| {
                    if tex.generation != l.generation || tex.residency.resident(l.tile) {
                        return None;
                    }
                    let is_root = l.tile.level == tex.residency.layout.root;
                    let parent = (!is_root)
                        .then(|| tex.residency.slot(l.tile.parent()))
                        .flatten();
                    Some((tex.atlas, is_root, tex.residency.can_commit(l.tile), parent))
                })
            else {
                continue;
            };
            let data = match &l.data {
                Ok(d) => d,
                Err(why) => {
                    if is_root {
                        tracing::warn!("virtual texture {}: root tile failed: {why}", l.id);
                        if let Some(t) = self.textures[l.id as usize - 1].as_mut() {
                            t.failed = true;
                        }
                    }
                    continue;
                }
            };
            if !can_commit {
                // Its parent isn't resident yet: wait for it a while.
                if l.arrived + 60 > frame {
                    keep.push(l);
                }
                continue;
            }
            if budget == 0 && !is_root {
                keep.push(l);
                continue;
            }
            self.ensure_atlas(gpu, atlas);
            // The parent stays: it was just asked for.
            if let Some(ps) = parent_slot {
                self.pools[atlas].touch(ps, frame);
            }
            let owner = SlotOwner {
                texture: l.id,
                tile: l.tile,
                last_used: frame,
            };
            let slot = match self.pools[atlas].alloc(owner) {
                Some(s) => s,
                None => {
                    let textures = &self.textures;
                    let victim = self.pools[atlas].lru(frame, |o| {
                        textures[o.texture as usize - 1].as_ref().is_some_and(|t| {
                            o.tile.level < t.residency.layout.root && t.residency.is_leaf(o.tile)
                        })
                    });
                    let Some(v) = victim.and_then(|v| self.pools[atlas].owner(v).map(|o| (v, o)))
                    else {
                        // Everything resident is in use: drop this one (asked for again later).
                        continue;
                    };
                    if let Some(t) = self.textures[v.1.texture as usize - 1].as_mut() {
                        t.residency.evict(v.1.tile);
                    }
                    self.pools[atlas].release(v.0);
                    self.stats.evictions += 1;
                    match self.pools[atlas].alloc(owner) {
                        Some(s) => s,
                        None => continue,
                    }
                }
            };
            self.upload_tile(gpu, atlas, slot, data);
            if let Some(t) = self.textures[l.id as usize - 1].as_mut() {
                t.residency.commit(l.tile, slot);
                if let Some((mean, low)) = l.mean {
                    t.mean = mean;
                    t.low = low;
                    t.info_dirty = true;
                }
            }
            if !is_root {
                budget -= 1;
            }
            self.stats.tiles_loaded += 1;
            self.stats.tiles_loaded_total += 1;
        }
        self.ready = keep;
    }

    fn upload_tile(&self, gpu: &GpuShared, atlas: usize, slot: u32, data: &[u8]) {
        let (sx, sy) = (slot % self.per_row, slot / self.per_row);
        let mut offset = 0usize;
        for level in 0..self.levels {
            let size = SLOT >> level;
            let (bytes_per_row, rows, bytes) = if self.compressed {
                let blocks = size / 4;
                (blocks * 16, blocks, (blocks * blocks * 16) as usize)
            } else {
                (size * 4, size, (size * size * 4) as usize)
            };
            let Some(chunk) = data.get(offset..offset + bytes) else {
                return;
            };
            gpu.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.atlases[atlas].texture,
                    mip_level: level,
                    origin: wgpu::Origin3d {
                        x: (sx * SLOT) >> level,
                        y: (sy * SLOT) >> level,
                        z: 0,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                chunk,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(bytes_per_row),
                    rows_per_image: Some(rows),
                },
                wgpu::Extent3d {
                    width: size,
                    height: size,
                    depth_or_array_layers: 1,
                },
            );
            offset += bytes;
        }
    }

    /// Free textures nobody drew for a while.
    fn evict_unused(&mut self) {
        let frame = self.frame;
        for i in 0..MAX_TEXTURES {
            let stale = self.textures[i]
                .as_ref()
                .is_some_and(|t| t.last_used + EVICT_AFTER_FRAMES < frame);
            if stale {
                self.free_texture(i as u8 + 1);
            }
        }
        self.snapshots.retain(|(index, srgb), _| {
            self.by_key
                .keys()
                .any(|(k, s)| *s == *srgb && matches!(k, SourceKey::Image(i, _) if i == index))
        });
    }

    fn free_texture(&mut self, id: u8) {
        let Some(tex) = self.textures[id as usize - 1].take() else {
            return;
        };
        for (_, slot) in tex.residency.resident_tiles() {
            self.pools[tex.atlas].release(slot);
        }
        self.by_key.remove(&tex.key);
        self.page_ranges.retain(|r| r.0 != tex.page_offset);
        self.in_flight.retain(|k| k.0 != id);
        self.ready.retain(|l| l.id != id);
    }

    /// Room for `words` page table words: a gap between ranges, or the end (the buffer
    /// grows next frame: see `grow_data`).
    fn alloc_pages(&mut self, words: u32) -> u32 {
        let mut start = PAGE_BASE;
        let mut at = self.page_ranges.len();
        for (i, &(s, len)) in self.page_ranges.iter().enumerate() {
            if s - start >= words {
                at = i;
                break;
            }
            start = s + len;
        }
        if at == self.page_ranges.len() {
            self.data_words = self.data_words.max(start + words);
        }
        self.page_ranges.insert(at, (start, words));
        start
    }

    /// Grow the page table buffer to what the tables need (between frames, so texture sets
    /// built during a frame stay valid), which rebinds every texture set and re-uploads every
    /// table.
    fn grow_data(&mut self, gpu: &GpuShared) {
        if self.data_words <= self.data_capacity {
            return;
        }
        let mut size = self.data_capacity;
        while size < self.data_words {
            size *= 2;
        }
        self.data_capacity = size;
        self.data = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("virtual texture page tables"),
            size: size as u64 * 4,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.bind_generation += 1;
        for t in self.textures.iter_mut().flatten() {
            t.residency.mark_all();
            t.info_dirty = true;
        }
    }

    /// Upload changed page tables and records, and the header.
    fn upload_tables(&mut self, gpu: &GpuShared) {
        let header: [u32; HEADER_WORDS as usize] = [
            self.frame as u32,
            self.mip_bias.to_bits(),
            ((self.per_row * SLOT) as f32).to_bits(),
            self.per_row,
            self.levels,
            0,
            0,
            0,
        ];
        gpu.queue
            .write_buffer(&self.data, 0, bytemuck::cast_slice(&header));
        for (i, t) in self.textures.iter_mut().enumerate() {
            let Some(t) = t else { continue };
            if t.info_dirty {
                t.info_dirty = false;
                let l = &t.residency.layout;
                let mut info = [0u32; INFO_WORDS as usize];
                info[0] = l.width;
                info[1] = l.height;
                info[2] = l.root | (t.atlas as u32) << 8;
                for m in 0..MAX_LEVELS {
                    info[4 + m] = t.page_offset + l.level_offsets[m];
                }
                let h = t.mean.map(f32_to_f16);
                info[20] = h[0] as u32 | (h[1] as u32) << 16;
                info[21] = h[2] as u32 | (h[3] as u32) << 16;
                for (i, texel) in t.low.iter().enumerate() {
                    let h = texel.map(f32_to_f16);
                    info[LOW_WORD + 2 * i] = h[0] as u32 | (h[1] as u32) << 16;
                    info[LOW_WORD + 2 * i + 1] = h[2] as u32 | (h[3] as u32) << 16;
                }
                let at = (HEADER_WORDS + INFO_WORDS * i as u32) as u64 * 4;
                gpu.queue
                    .write_buffer(&self.data, at, bytemuck::cast_slice(&info));
            }
            if let Some((a, b)) = t.residency.take_dirty() {
                let at = (t.page_offset as u64 + a as u64) * 4;
                gpu.queue.write_buffer(
                    &self.data,
                    at,
                    bytemuck::cast_slice(&t.residency.table[a..b]),
                );
            }
        }
    }

    fn update_stats(&mut self) {
        let slot_bytes = self.slot_bytes();
        let s = &mut self.stats;
        s.textures = self.textures.iter().flatten().count() as u32;
        s.resident_tiles = self.pools.iter().map(SlotPool::used).sum();
        let real = self.atlases.iter().filter(|a| a.real).count() as u64;
        s.cache_tiles = (real * self.capacity as u64) as u32;
        let side = (self.per_row * SLOT) as u64;
        let mut atlas_bytes = 0;
        for l in 0..self.levels {
            let d = side >> l;
            atlas_bytes += if self.compressed { d * d } else { d * d * 4 };
        }
        s.cache_bytes = real * atlas_bytes;
        s.resident_bytes = s.resident_tiles as u64 * slot_bytes;
        s.pending = (self.in_flight.len() + self.ready.len()) as u32;
        s.compressed = self.compressed;
        s.feedback = self.feedback_writes;
        if self.frame.is_multiple_of(8) || s.resident_by_level.is_empty() {
            let mut by = vec![0u32; MAX_LEVELS];
            for t in self.textures.iter().flatten() {
                for (tile, _) in t.residency.resident_tiles() {
                    by[tile.level as usize] += 1;
                }
            }
            while by.len() > 1 && by.last() == Some(&0) {
                by.pop();
            }
            s.resident_by_level = by;
        }
    }

    /// The virtual texture id to draw `handle` with in a slot (`color`: an sRGB view of sRGB
    /// images), or `None` when it is an ordinary texture or not ready to draw yet (the caller
    /// then binds the image itself as usual). Registers images over the size threshold and
    /// textures registered with [`VirtualTextures`] on first use and starts loading their root
    /// tile.
    pub fn texture_for(
        &mut self,
        images: &Assets<Image>,
        registry: Option<&VirtualTextures>,
        handle: Handle<Image>,
        color: bool,
        settings: &RenderSettings,
    ) -> Option<u8> {
        if !settings.virtual_textures {
            return None;
        }
        let index = handle.index();
        let (key, provider): (SourceKey, Option<Arc<dyn TileProvider>>) =
            if let Some(reg) = registry.filter(|r| r.is_registered(index)) {
                let (provider, generation) = reg.provider(index)?;
                (SourceKey::Registered(index, generation), Some(provider))
            } else {
                let image = images.get(handle)?;
                let threshold = settings.virtual_texture_threshold.max(TILE);
                if (image.width <= threshold && image.height <= threshold)
                    || image.format != PixelFormat::Rgba8
                {
                    return None;
                }
                let version = images.version(handle);
                if self.rejected.get(&index) == Some(&version) {
                    return None;
                }
                (SourceKey::Image(index, version), None)
            };
        let srgb_view = color
            && match &provider {
                Some(p) => p.srgb(),
                None => images.get(handle).is_some_and(|i| i.srgb),
            };
        let id = match self.by_key.get(&(key, srgb_view)) {
            Some(&id) => id,
            None => {
                let provider = match provider {
                    Some(p) => p,
                    None => match self.image_provider(images, handle, srgb_view) {
                        Ok(p) => p,
                        Err(why) => {
                            if let Some(image) = images.get(handle) {
                                tracing::warn!(
                                    "image {index} ({}x{}) is over the virtual texture threshold \
                                     but can't be virtual ({why}); drawing it as a whole texture",
                                    image.width,
                                    image.height
                                );
                            }
                            self.rejected.insert(index, images.version(handle));
                            return None;
                        }
                    },
                };
                self.create((key, srgb_view), provider, srgb_view)?
            }
        };
        let tex = self.textures[id as usize - 1].as_mut()?;
        tex.last_used = self.frame;
        if tex.failed {
            return None;
        }
        if !tex.residency.ready() {
            self.request_root(id);
            return None;
        }
        Some(id)
    }

    /// Whether `handle` is an image over the size threshold that will be drawn virtual once its
    /// root tile is in: the caller binds nothing for it meanwhile, rather than uploading the
    /// whole image.
    pub fn is_pending_image(
        &self,
        images: &Assets<Image>,
        registry: Option<&VirtualTextures>,
        handle: Handle<Image>,
        settings: &RenderSettings,
    ) -> bool {
        let index = handle.index();
        if !settings.virtual_textures || registry.is_some_and(|r| r.is_registered(index)) {
            return false;
        }
        let Some(image) = images.get(handle) else {
            return false;
        };
        let threshold = settings.virtual_texture_threshold.max(TILE);
        let version = images.version(handle);
        let over = (image.width > threshold || image.height > threshold)
            && image.format == PixelFormat::Rgba8;
        let failed = [false, true].iter().any(|&srgb| {
            self.by_key
                .get(&(SourceKey::Image(index, version), srgb))
                .and_then(|&id| self.textures[id as usize - 1].as_ref())
                .is_some_and(|t| t.failed)
        });
        over && !failed && self.rejected.get(&index) != Some(&version)
    }

    fn image_provider(
        &mut self,
        images: &Assets<Image>,
        handle: Handle<Image>,
        srgb: bool,
    ) -> Result<Arc<dyn TileProvider>, String> {
        let index = handle.index();
        let version = images.version(handle);
        if let Some((v, p)) = self.snapshots.get(&(index, srgb))
            && *v == version
        {
            return Ok(p.clone());
        }
        let image = images.get(handle).ok_or("not loaded")?;
        if image.is_released() {
            return Err("its pixels were released".into());
        }
        // Old versions of this image go.
        let old: Vec<u8> = self
            .by_key
            .iter()
            .filter(
                |((k, _), _)| matches!(k, SourceKey::Image(i, v) if *i == index && *v != version),
            )
            .map(|(_, id)| *id)
            .collect();
        for id in old {
            self.free_texture(id);
        }
        let p = Arc::new(ImageProvider::new(image.clone(), srgb)?);
        self.snapshots.insert((index, srgb), (version, p.clone()));
        Ok(p)
    }

    fn create(
        &mut self,
        key: (SourceKey, bool),
        provider: Arc<dyn TileProvider>,
        srgb_view: bool,
    ) -> Option<u8> {
        let (w, h) = provider.size();
        let layout = match Layout::new(w, h) {
            Ok(l) => l,
            Err(why) => {
                tracing::warn!("virtual texture can't be used: {why}");
                if let SourceKey::Image(index, version) = key.0 {
                    self.rejected.insert(index, version);
                }
                return None;
            }
        };
        // Replaced registrations of the same image go.
        if let SourceKey::Registered(index, generation) = key.0 {
            let old: Vec<u8> = self
                .by_key
                .iter()
                .filter(|((k, _), _)| {
                    matches!(k, SourceKey::Registered(i, g) if *i == index && *g != generation)
                })
                .map(|(_, id)| *id)
                .collect();
            for id in old {
                self.free_texture(id);
            }
        }
        let slot = self.textures.iter().position(Option::is_none)?;
        let id = slot as u8 + 1;
        let page_offset = self.alloc_pages(layout.entries);
        self.next_generation = self.next_generation.wrapping_add(1);
        let generation = self.next_generation;
        self.textures[slot] = Some(VirtualTex {
            key,
            provider,
            residency: TextureResidency::new(layout),
            atlas: if srgb_view { ATLAS_SRGB } else { ATLAS_LINEAR },
            page_offset,
            last_used: self.frame,
            generation,
            mean: [0.5, 0.5, 0.5, 1.0],
            low: [[0.5, 0.5, 0.5, 1.0]; 16],
            info_dirty: true,
            root_requested: false,
            failed: false,
        });
        self.by_key.insert(key, id);
        Some(id)
    }

    /// Copy this frame's feedback for reading back and clear it for the next frame (the
    /// mesh feature's `compute`, before the prepass writes it again).
    pub fn encode(&mut self, encoder: &mut wgpu::CommandEncoder) {
        if !self.feedback_writes || self.textures.iter().all(Option::is_none) {
            return;
        }
        if let Some(slot) = self
            .readbacks
            .iter()
            .position(|r| r.state.load(Ordering::Acquire) == FREE)
        {
            encoder.copy_buffer_to_buffer(
                &self.feedback,
                0,
                &self.readbacks[slot].buffer,
                0,
                FEEDBACK_ENTRIES as u64 * 4,
            );
            self.readbacks[slot]
                .state
                .store(IN_FLIGHT, Ordering::Release);
            self.written = Some(slot);
        }
        encoder.clear_buffer(&self.feedback, 0, None);
    }

    /// State for `render.stats`.
    pub fn describe(&self) -> serde_json::Value {
        serde_json::json!({
            "stats": self.stats,
            "cache_side_texels": self.per_row * SLOT,
            "slot_levels": self.levels,
            "tile": TILE,
            "border": BORDER,
        })
    }
}

/// Mean color and 4×4 box-filtered level of a root tile whose level is `size` texels (at
/// most 128² inside the border), decoded to linear light for sRGB color (what the shader gets
/// through the sRGB cache). The 4×4 level is what a whole texture's mip chain has a few
/// levels past the root slot's tail.
fn root_summary(tile: &[u8], srgb: bool, size: (u32, u32)) -> ([f32; 4], [[f32; 4]; 16]) {
    let decode = |v: u8, c: usize| {
        let v = v as f64 / 255.0;
        if srgb && c < 3 {
            if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        } else {
            v
        }
    };
    // Cell bounds: a quarter of the level each way, at least one texel.
    let span = |n: u32, i: u32| {
        let a = (i * n / 4).min(n - 1);
        (a, ((i + 1) * n / 4).max(a + 1))
    };
    let mut low = [[0.0f32; 4]; 16];
    let mut total = [0.0f64; 4];
    for cy in 0..4 {
        let (y0, y1) = span(size.1, cy);
        for cx in 0..4 {
            let (x0, x1) = span(size.0, cx);
            let mut sum = [0.0f64; 4];
            for y in y0..y1 {
                for x in x0..x1 {
                    let o = (((BORDER + y) * SLOT + BORDER + x) * 4) as usize;
                    for (c, s) in sum.iter_mut().enumerate() {
                        *s += decode(tile[o + c], c);
                    }
                }
            }
            let n = ((y1 - y0) * (x1 - x0)) as f64;
            low[(cy * 4 + cx) as usize] = sum.map(|s| (s / n) as f32);
        }
    }
    for y in 0..size.1 {
        for x in 0..size.0 {
            let o = (((BORDER + y) * SLOT + BORDER + x) * 4) as usize;
            for (c, s) in total.iter_mut().enumerate() {
                *s += decode(tile[o + c], c);
            }
        }
    }
    let n = (size.0 * size.1) as f64;
    (total.map(|s| (s / n) as f32), low)
}

/// Whether fragment shaders write tile feedback on this device: Vulkan, Metal and DX12 write
/// storage buffers from fragment shaders; OpenGL ES may not.
pub fn feedback_writes(gpu: &GpuShared) -> bool {
    gpu.adapter_info.backend != wgpu::Backend::Gl
}

/// The optional parts of the mesh shaders a pipeline is compiled with (WGSL overrides
/// `VT_ENABLED` and `ANTI_TILING_ENABLED`). Both start off and turn on for good once a frame
/// draws a material that needs them: software rasterizers (lavapipe) compile every pipeline's
/// whole shader, and these parts make the mesh shaders several times larger, so scenes
/// without virtual textures or anti-tiled materials would pay for them in every pipeline.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ShaderFeatures {
    pub virtual_textures: bool,
    pub anti_tiling: bool,
}

impl ShaderFeatures {
    /// Both on.
    pub const ALL: Self = Self {
        virtual_textures: true,
        anti_tiling: true,
    };

    /// Everything either has.
    pub fn union(self, other: Self) -> Self {
        Self {
            virtual_textures: self.virtual_textures || other.virtual_textures,
            anti_tiling: self.anti_tiling || other.anti_tiling,
        }
    }

    /// The overrides for `mesh_source` (both).
    pub fn mesh_constants(self) -> [(&'static str, f64); 2] {
        [
            ("VT_ENABLED", f64::from(u8::from(self.virtual_textures))),
            ("ANTI_TILING_ENABLED", f64::from(u8::from(self.anti_tiling))),
        ]
    }

    /// The overrides for `mesh_shadow_source` (virtual textures only).
    pub fn shadow_constants(self) -> [(&'static str, f64); 1] {
        [("VT_ENABLED", f64::from(u8::from(self.virtual_textures)))]
    }
}

/// `mesh.wgsl` with the virtual texture library in front of it.
pub fn mesh_source(feedback_writes: bool) -> String {
    format!(
        "{}\n{}",
        shader_source(feedback_writes),
        include_str!("../shaders/mesh.wgsl")
    )
}

/// `mesh_shadow.wgsl` with the virtual texture library in front of it.
pub fn mesh_shadow_source(feedback_writes: bool) -> String {
    format!(
        "{}\n{}",
        shader_source(feedback_writes),
        include_str!("../shaders/mesh_shadow.wgsl")
    )
}

/// The WGSL of the virtual texture library, with feedback writes compiled out on devices
/// without writable storage in fragment shaders.
pub fn shader_source(feedback_writes: bool) -> String {
    let src = include_str!("../shaders/virtual_texture.wgsl");
    if feedback_writes {
        return src.to_string();
    }
    let begin = src.find("// VT_FEEDBACK_BEGIN").unwrap_or(0);
    let end = src
        .find("// VT_FEEDBACK_END")
        .map_or(src.len(), |e| e + "// VT_FEEDBACK_END".len());
    format!(
        "{}@group(2) @binding(11) var<storage, read> vt_feedback: array<u32>;\nfn vt_feedback_store(key: u32) {{}}\n{}",
        &src[..begin],
        &src[end..]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_summary_box_filters_the_level_to_four_by_four() {
        // A 64×32 root level (inside the border): left half 255, right half 0, in the red
        // channel; linear data.
        let mut tile = vec![0u8; (SLOT * SLOT * 4) as usize];
        for y in 0..32 {
            for x in 0..32 {
                tile[(((BORDER + y) * SLOT + BORDER + x) * 4) as usize] = 255;
            }
        }
        let (mean, low) = root_summary(&tile, false, (64, 32));
        assert!((mean[0] - 0.5).abs() < 1e-6, "mean {mean:?}");
        for cy in 0..4 {
            for cx in 0..4 {
                let expect = if cx < 2 { 1.0 } else { 0.0 };
                assert_eq!(low[cy * 4 + cx][0], expect, "cell ({cx}, {cy})");
            }
        }
    }

    #[test]
    fn the_shader_record_layout_matches() {
        let src = include_str!("../shaders/virtual_texture.wgsl");
        assert!(src.contains(&format!("const VT_INFO_WORDS: u32 = {INFO_WORDS}u;")));
        assert!(src.contains(&format!("const VT_LOW_WORD: u32 = {LOW_WORD}u;")));
        assert!(src.contains(&format!("const VT_HEADER_WORDS: u32 = {HEADER_WORDS}u;")));
    }
}
