//! The ray tiles (binding 5 of the scene's group): a material's base colour at 128 texels
//! across with every level down to its mean, a layer of a 256-layer array each, which a hit
//! samples at its uv (`ray_surface` in lib_ray.wgsl, which divides by the layer's last level:
//! the row holds the texture's mean already, so a texture counts once). A layer is filled on
//! the GPU from the texture the material is drawn with (the image cache's upload, the virtual
//! texture cache's root tile, a preset's generated map) when the material first enters the
//! scene: the nearest asks first, a few a frame, after the scene's builds. A layer no row has
//! named for a while goes to another texture, the longest unnamed first; a material without a
//! layer is lit with its row's colour. The array is made at the first fill (a scene without
//! textures never makes it) and goes with the scene.

use std::collections::HashMap;

use wgpu::util::DeviceExt;

use super::blas::Name;
use super::hook::RayTileSource;

const SHADER: &str = include_str!("../shaders/ray_tiles.wgsl");
/// Texels across a layer's first level, and its levels, down to one texel (the mean
/// `ray_surface` divides by).
const SIZE: u32 = 128;
const MIPS: u32 = 8;
/// Layers of the array.
pub(super) const LAYERS: u16 = 256;
/// RGBA8 holding sRGB-encoded values (a compute pass can't write an sRGB format), read through
/// an sRGB view.
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const VIEW_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
/// Layers filled a frame, the nearest asks first.
const FILLS_PER_FRAME: usize = 4;
/// Frames between two looks at whether a held texture's pixels changed.
const CHECK_FRAMES: u64 = 64;
/// Frames an ask lasts without being asked again (a placed slot is looked at every 64 frames
/// at least: `ROLL_FRAMES` in mesh_feature/rays.rs).
const ASK_FRAMES: u64 = 128;
/// Frames a layer no row names keeps its texture before another may take it: far longer than
/// a structure goes on holding an instance that left (its slot is emptied at the structure's
/// next build), whose row a ray may read until then.
const IDLE_FRAMES: u64 = 128;

/// `render.ray.tiles*`.
#[derive(Clone, Copy, Default, Debug)]
pub(super) struct TileStats {
    /// Layers holding a texture; asks waiting for a layer (or for new pixels in theirs);
    /// layers filled this frame; layers given to another texture since the scene was made;
    /// MiB of the array (0 until the first fill makes it).
    pub layers: u32,
    pub waiting: u32,
    pub filled: u32,
    pub evicted: u32,
    pub mb: f32,
}

struct Layer {
    name: Name,
    version: u64,
    /// The last frame a row named it.
    used: u64,
    /// The last frame its source was asked whether its pixels changed.
    checked: u64,
}

struct Ask<S> {
    source: S,
    version: u64,
    /// Its pixels stay where they are while `source` is held (`RayTileSource::lasting`), so it
    /// may be filled in a later frame than it was found in.
    lasting: bool,
    /// The nearest instance that asked in the frame `asked` (meters).
    distance: f32,
    asked: u64,
    /// The layer it fills again (its pixels changed); `None`: a layer of its own.
    refill: Option<u16>,
}

/// Which texture each layer holds and which wait for one. No GPU in it.
pub(super) struct Book<S> {
    most: u16,
    layers: Vec<Option<Layer>>,
    named: HashMap<Name, u16>,
    asks: HashMap<Name, Ask<S>>,
    /// Layers given to another texture so far.
    evicted: u32,
}

impl<S> Book<S> {
    pub(super) fn new(most: u16) -> Self {
        Self {
            most,
            layers: Vec::new(),
            named: HashMap::new(),
            asks: HashMap::new(),
            evicted: 0,
        }
    }

    /// See `RaySceneOut::tile`; `source` gives the pixels, their version and whether they are
    /// lasting, asked at an ask's first call of a frame (where its pixels are now: a cache may
    /// since have put another texture's where they were), and every `CHECK_FRAMES` for a
    /// texture a layer holds.
    pub(super) fn ask(
        &mut self,
        name: Name,
        distance: f32,
        frame: u64,
        source: impl FnOnce() -> Option<(S, u64, bool)>,
    ) -> Option<u16> {
        if let Some(&layer) = self.named.get(&name) {
            let held = self.layers[layer as usize].as_mut().expect("a named layer");
            let waiting = self.asks.get(&name).map(|ask| ask.asked);
            if waiting == Some(frame) {
                let ask = self.asks.get_mut(&name).expect("waiting");
                ask.distance = ask.distance.min(distance);
            } else if waiting.is_some() || frame >= held.checked + CHECK_FRAMES {
                held.checked = frame;
                match source() {
                    // New pixels: filled into its layer, which its rows go on naming meanwhile.
                    Some((source, version, lasting)) if version != held.version => {
                        let ask = Ask {
                            source,
                            version,
                            lasting,
                            distance,
                            asked: frame,
                            refill: Some(layer),
                        };
                        self.asks.insert(name, ask);
                    }
                    // The pixels it holds, or none on the GPU: the layer keeps what it has.
                    _ => {
                        self.asks.remove(&name);
                    }
                }
            }
            return Some(layer);
        }
        match self.asks.get_mut(&name) {
            Some(ask) if ask.asked == frame => ask.distance = ask.distance.min(distance),
            // A new ask, or its first call of a frame: where its pixels are now (a view kept
            // from frames ago might show what a cache has since put in its place).
            _ => match source() {
                Some((source, version, lasting)) => {
                    let ask = Ask {
                        source,
                        version,
                        lasting,
                        distance,
                        asked: frame,
                        refill: None,
                    };
                    self.asks.insert(name, ask);
                }
                None => {
                    self.asks.remove(&name);
                }
            },
        }
        None
    }

    /// The frame's fills, the nearest asks first, each with its layer: its own for new pixels
    /// of a texture it holds, else a free one, else the one no row has named the longest (for
    /// `IDLE_FRAMES` at least). An ask whose pixels are a place in a cache is filled only in
    /// the frame it was found in (the cache may give that place to another texture after),
    /// else at its next ask, where they are then. `users`: the rows that name a layer now.
    pub(super) fn settle(&mut self, users: impl Fn(u16) -> u32, frame: u64) -> Vec<(u16, S)> {
        self.asks.retain(|_, ask| ask.asked + ASK_FRAMES > frame);
        for (i, layer) in self.layers.iter_mut().enumerate() {
            if let Some(layer) = layer
                && users(i as u16) > 0
            {
                layer.used = frame;
            }
        }
        let mut order: Vec<(f32, Name)> = self
            .asks
            .iter()
            .filter(|(_, ask)| ask.lasting || ask.asked == frame)
            .map(|(&name, ask)| (ask.distance, name))
            .collect();
        order.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut fills = Vec::new();
        // No layer is free for a new texture this frame (refills go on).
        let mut full = false;
        for (_, name) in order {
            if fills.len() == FILLS_PER_FRAME {
                break;
            }
            let refill = self.asks[&name]
                .refill
                .filter(|&layer| self.named.get(&name) == Some(&layer));
            let layer = match refill {
                Some(layer) => layer,
                None if full => continue,
                None => match self.free_layer(&users, frame) {
                    Some(layer) => layer,
                    None => {
                        full = true;
                        continue;
                    }
                },
            };
            let ask = self.asks.remove(&name).expect("asked");
            let now = Layer {
                name,
                version: ask.version,
                used: frame,
                checked: frame,
            };
            if let Some(was) = self.layers[layer as usize].replace(now)
                && was.name != name
            {
                self.named.remove(&was.name);
                self.evicted += 1;
            }
            self.named.insert(name, layer);
            fills.push((layer, ask.source));
        }
        fills
    }

    fn free_layer(&mut self, users: &impl Fn(u16) -> u32, frame: u64) -> Option<u16> {
        if let Some(i) = self.layers.iter().position(Option::is_none) {
            return Some(i as u16);
        }
        if self.layers.len() < self.most as usize {
            self.layers.push(None);
            return Some(self.layers.len() as u16 - 1);
        }
        self.layers
            .iter()
            .enumerate()
            .filter_map(|(i, layer)| Some((i as u16, layer.as_ref()?)))
            .filter(|&(i, layer)| users(i) == 0 && frame >= layer.used + IDLE_FRAMES)
            .min_by_key(|&(_, layer)| layer.used)
            .map(|(i, _)| i)
    }

    /// Layers that hold a texture; asks waiting.
    fn counts(&self) -> (u32, u32) {
        (self.named.len() as u32, self.asks.len() as u32)
    }
}

/// Where a fill reads (a [`RayTileSource`] without its version).
struct Source {
    view: wgpu::TextureView,
    rect: [f32; 4],
}

/// The array and what fills it, made at the first fill.
struct Gpu {
    texture: wgpu::Texture,
    layout: wgpu::BindGroupLayout,
    pipeline: wgpu::ComputePipeline,
    sampler: wgpu::Sampler,
    /// `[0, 0, 1, 1]`: a level's rect in the level above.
    whole: wgpu::Buffer,
}

impl Gpu {
    fn new(device: &wgpu::Device) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("ray tiles"),
            size: wgpu::Extent3d {
                width: SIZE,
                height: SIZE,
                depth_or_array_layers: LAYERS as u32,
            },
            mip_level_count: MIPS,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            // COPY_DST and no RENDER_ATTACHMENT: wgpu zeroes it by buffer copies, not by a
            // render pass a level and layer through 2,048 views it would make now. COPY_SRC:
            // the tests read it back.
            usage: wgpu::TextureUsages::STORAGE_BINDING
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[VIEW_FORMAT],
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("ray tiles"),
            source: wgpu::ShaderSource::Wgsl(crate::features::compose_shader(SHADER, false).into()),
        });
        let entry =
            |binding, ty| crate::renderer::layout_entry(binding, wgpu::ShaderStages::COMPUTE, ty);
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ray tiles"),
            entries: &[
                entry(
                    0,
                    wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                ),
                entry(
                    1,
                    wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                ),
                entry(
                    2,
                    wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                ),
                entry(
                    3,
                    wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: FORMAT,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                ),
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("ray tiles"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("cs_tile_fill"),
            layout: Some(&pipeline_layout),
            module: &module,
            entry_point: Some("cs_tile_fill"),
            compilation_options: Default::default(),
            cache: None,
        });
        // As the frame's `linear_repeat`: textures repeat.
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("ray tiles"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            address_mode_w: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let whole = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("ray tiles whole"),
            contents: bytemuck::bytes_of(&[0.0f32, 0.0, 1.0, 1.0]),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        Self {
            texture,
            layout,
            pipeline,
            sampler,
            whole,
        }
    }

    /// A view of level `mip` of layer `layer` alone.
    fn level(
        &self,
        layer: u16,
        mip: u32,
        format: wgpu::TextureFormat,
        usage: wgpu::TextureUsages,
    ) -> wgpu::TextureView {
        self.texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("ray tile level"),
            format: Some(format),
            dimension: Some(wgpu::TextureViewDimension::D2),
            usage: Some(usage),
            base_mip_level: mip,
            mip_level_count: Some(1),
            base_array_layer: layer as u32,
            array_layer_count: Some(1),
            ..Default::default()
        })
    }

    fn fill_group(
        &self,
        device: &wgpu::Device,
        source: &wgpu::TextureView,
        rect: &wgpu::Buffer,
        out: &wgpu::TextureView,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ray tile fill"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(source),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: rect.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(out),
                },
            ],
        })
    }
}

/// The ray tiles of a scene.
pub(super) struct Tiles {
    book: Book<Source>,
    /// What binding 5 binds: the 1x1 stand-in until the first fill makes the array.
    view: wgpu::TextureView,
    gpu: Option<Gpu>,
    stats: TileStats,
}

impl Tiles {
    pub(super) fn new(stand_in: wgpu::TextureView) -> Self {
        Self {
            book: Book::new(LAYERS),
            view: stand_in,
            gpu: None,
            stats: TileStats::default(),
        }
    }

    pub(super) fn view(&self) -> &wgpu::TextureView {
        &self.view
    }

    pub(super) fn stats(&self) -> TileStats {
        self.stats
    }

    /// `RaySceneOut::tile`.
    pub(super) fn ask(
        &mut self,
        name: Name,
        distance: f32,
        frame: u64,
        source: impl FnOnce() -> Option<RayTileSource>,
    ) -> Option<u16> {
        self.book.ask(name, distance, frame, || {
            source().map(|s| {
                let source = Source {
                    view: s.view,
                    rect: s.rect,
                };
                (source, s.version, s.lasting)
            })
        })
    }

    /// Record the frame's fills, after the scene's builds: each a layer's 8 levels, a dispatch
    /// each, in one pass. `users`: the rows naming a layer now. True when the array was made
    /// now: the scene's group binds its view from now on.
    pub(super) fn encode(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        users: impl Fn(u16) -> u32,
        frame: u64,
    ) -> bool {
        let fills = self.book.settle(users, frame);
        (self.stats.layers, self.stats.waiting) = self.book.counts();
        self.stats.evicted = self.book.evicted;
        self.stats.filled = fills.len() as u32;
        if fills.is_empty() {
            return false;
        }
        let made = self.gpu.is_none();
        let gpu: &Gpu = self.gpu.get_or_insert_with(|| Gpu::new(device));
        if made {
            self.view = gpu.texture.create_view(&wgpu::TextureViewDescriptor {
                label: Some("ray tiles"),
                format: Some(VIEW_FORMAT),
                dimension: Some(wgpu::TextureViewDimension::D2Array),
                // (An sRGB view can't be a storage binding.)
                usage: Some(wgpu::TextureUsages::TEXTURE_BINDING),
                ..Default::default()
            });
            let layer: u64 = (0..MIPS).map(|m| ((SIZE >> m) as u64).pow(2) * 4).sum();
            self.stats.mb = (layer * LAYERS as u64) as f32 / (1u64 << 20) as f32;
        }
        // Every group first, then the pass.
        let mut runs: Vec<(wgpu::BindGroup, u32)> = Vec::with_capacity(fills.len() * MIPS as usize);
        for (layer, source) in &fills {
            let rect = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("ray tile rect"),
                contents: bytemuck::bytes_of(&source.rect),
                usage: wgpu::BufferUsages::UNIFORM,
            });
            for mip in 0..MIPS {
                let out = gpu.level(*layer, mip, FORMAT, wgpu::TextureUsages::STORAGE_BINDING);
                let group = if mip == 0 {
                    gpu.fill_group(device, &source.view, &rect, &out)
                } else {
                    // The level above, decoded before it is filtered.
                    let above = gpu.level(
                        *layer,
                        mip - 1,
                        VIEW_FORMAT,
                        wgpu::TextureUsages::TEXTURE_BINDING,
                    );
                    gpu.fill_group(device, &above, &gpu.whole, &out)
                };
                runs.push((group, (SIZE >> mip).max(1).div_ceil(8)));
            }
        }
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("ray tiles"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&gpu.pipeline);
        for (group, n) in &runs {
            pass.set_bind_group(0, group, &[]);
            pass.dispatch_workgroups(*n, *n, 1);
        }
        made
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ray_scene::RayKey;

    fn name(n: u64) -> Name {
        (0, RayKey(n, 0))
    }

    /// A source of `n`'s pixels at `version` in a texture of their own, and in a cache's place.
    fn own(n: u64, version: u64) -> Option<(u64, u64, bool)> {
        Some((n, version, true))
    }

    fn cached(n: u64, version: u64) -> Option<(u64, u64, bool)> {
        Some((n, version, false))
    }

    #[test]
    fn shader_validates() {
        let code = crate::features::compose_shader(SHADER, false);
        crate::features::validate_wgsl("ray tiles", &code).unwrap();
    }

    /// The nearest asks get layers first, a few a frame; a layer stays with its texture while
    /// a row names it, and goes to another only once no row has named it for `IDLE_FRAMES`,
    /// the longest unnamed first; new pixels are filled into the layer that has the texture.
    #[test]
    fn the_nearest_textures_get_layers_and_the_longest_unnamed_layer_is_taken() {
        let mut book: Book<u64> = Book::new(5);
        let none = |_: u16| 0u32;
        // Six textures 6 m .. 1 m away: the four nearest, into layers 0-3.
        for n in 0..6 {
            assert_eq!(book.ask(name(n), 6.0 - n as f32, 1, || own(n, 0)), None);
        }
        assert_eq!(book.settle(none, 1), [(0, 5), (1, 4), (2, 3), (3, 2)]);
        // Then the next nearest into the last free layer; the farthest waits.
        for n in 2..6 {
            assert!(book.ask(name(n), 1.0, 2, || own(n, 0)).is_some());
        }
        for n in 0..2 {
            assert_eq!(book.ask(name(n), 6.0 - n as f32, 2, || own(n, 0)), None);
        }
        assert_eq!(book.settle(none, 2), [(4, 1)]);
        assert_eq!(book.counts(), (5, 1));
        // From frame 3 every layer but 2 (texture 3, filled at frame 1) is named by a row.
        let named = |layer: u16| (layer != 2) as u32;
        for frame in 3..1 + IDLE_FRAMES {
            book.ask(name(0), 6.0, frame, || own(0, 0));
            assert!(book.settle(named, frame).is_empty(), "frame {frame}");
        }
        book.ask(name(0), 6.0, 1 + IDLE_FRAMES, || own(0, 0));
        assert_eq!(book.evicted, 0);
        assert_eq!(book.settle(named, 1 + IDLE_FRAMES), [(2, 0)]);
        assert_eq!(book.evicted, 1);
        let at = 2 + IDLE_FRAMES;
        assert_eq!(
            book.ask(name(3), 3.0, at, || own(3, 0)),
            None,
            "texture 3 lost its layer"
        );
        // Texture 5's new pixels are seen at its next look and filled into its layer.
        assert_eq!(book.ask(name(5), 1.0, at, || own(50, 1)), Some(0));
        assert_eq!(book.settle(named, at), [(0, 50)]);
        // An ask nobody renews is dropped.
        assert_eq!(book.counts(), (5, 1));
        assert!(book.settle(named, at + ASK_FRAMES).is_empty());
        assert_eq!(book.counts(), (5, 0));
    }

    /// While no layer is free, new pixels for a texture a layer holds are still filled.
    #[test]
    fn new_pixels_are_filled_while_every_layer_is_taken() {
        let mut book: Book<u64> = Book::new(1);
        let all = |_: u16| 1u32;
        assert_eq!(book.ask(name(1), 1.0, 1, || own(1, 0)), None);
        assert_eq!(book.settle(all, 1), [(0, 1)]);
        // A nearer texture waits for the one layer; the held one gets new pixels.
        let at = 1 + CHECK_FRAMES;
        assert_eq!(book.ask(name(2), 0.5, at, || own(2, 0)), None);
        assert_eq!(book.ask(name(1), 3.0, at, || own(10, 1)), Some(0));
        assert_eq!(book.settle(all, at), [(0, 10)]);
        assert_eq!(book.counts(), (1, 1));
    }

    /// An ask whose pixels are a place in a cache is filled only in the frame it was found in
    /// (the place may hold another texture's pixels after), and found again at its next ask;
    /// one whose pixels are a texture of their own waits for a layer as long as it lasts.
    #[test]
    fn a_place_in_a_cache_is_filled_only_in_the_frame_it_was_found_in() {
        let mut book: Book<u64> = Book::new(8);
        let none = |_: u16| 0u32;
        // Six textures 1 m .. 6 m away, the one at 5 m in a cache: the four nearest are filled.
        for n in 0..6 {
            let found = if n == 4 { cached(n, 0) } else { own(n, 0) };
            assert_eq!(book.ask(name(n), 1.0 + n as f32, 1, || found), None);
        }
        assert_eq!(book.settle(none, 1), [(0, 0), (1, 1), (2, 2), (3, 3)]);
        // Not asked at frame 2: the one of its own is filled, the cached one waits.
        assert_eq!(book.settle(none, 2), [(4, 5)]);
        assert_eq!(book.counts(), (5, 1));
        assert!(book.settle(none, 3).is_empty());
        // Asked again, from where its pixels are now.
        assert_eq!(book.ask(name(4), 5.0, 4, || cached(40, 0)), None);
        assert_eq!(book.settle(none, 4), [(5, 40)]);
        assert_eq!(book.counts(), (6, 0));
    }

    /// New pixels of a texture a layer holds that wait for their fill are looked for again at
    /// each frame's first ask (a cache's place serves only its frame); gone from the GPU, the
    /// layer keeps what it has.
    #[test]
    fn new_pixels_are_looked_for_again_while_they_wait() {
        let mut book: Book<u64> = Book::new(16);
        let none = |_: u16| 0u32;
        assert_eq!(book.ask(name(9), 9.0, 1, || cached(9, 0)), None);
        assert_eq!(book.settle(none, 1), [(0, 9)]);
        // Its new pixels are seen at its next look, but four nearer textures fill first.
        let at = 1 + CHECK_FRAMES;
        for n in 0..4 {
            assert_eq!(book.ask(name(n), 1.0 + n as f32, at, || own(n, 0)), None);
        }
        assert_eq!(book.ask(name(9), 9.0, at, || cached(90, 1)), Some(0));
        assert_eq!(book.settle(none, at), [(1, 0), (2, 1), (3, 2), (4, 3)]);
        // The next frame they are where the cache has them then.
        assert_eq!(book.ask(name(9), 9.0, at + 1, || cached(91, 1)), Some(0));
        assert_eq!(book.settle(none, at + 1), [(0, 91)]);
        // Newer pixels again, which wait behind four nearer textures and are not asked for
        // the next frame: the place found is not filled from.
        let next = at + 1 + CHECK_FRAMES;
        for n in 10..14 {
            assert_eq!(book.ask(name(n), (n - 9) as f32, next, || own(n, 0)), None);
        }
        assert_eq!(book.ask(name(9), 9.0, next, || cached(92, 2)), Some(0));
        assert_eq!(
            book.settle(none, next),
            [(5, 10), (6, 11), (7, 12), (8, 13)]
        );
        assert!(book.settle(none, next + 1).is_empty());
        // Asked again, gone from the GPU: its layer keeps the pixels it has.
        assert_eq!(book.ask(name(9), 9.0, next + 2, || None), Some(0));
        assert!(book.settle(none, next + 2).is_empty());
        assert_eq!(book.counts(), (9, 0));
    }

    fn device() -> Option<crate::gpu::Gpu> {
        match crate::gpu::create(None, 1, 1, false) {
            Ok((gpu, _)) => Some(gpu),
            Err(e) => {
                eprintln!("skipping: no GPU ({e})");
                None
            }
        }
    }

    fn srgb_to_linear(c: u8) -> f32 {
        let c = c as f32 / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    }

    /// Level `mip` of layer `layer` of the array as stored (sRGB-encoded RGBA8).
    fn read_level(gpu: &crate::gpu::Gpu, tiles: &Tiles, layer: u16, mip: u32) -> Vec<[u8; 4]> {
        let side = (SIZE >> mip).max(1);
        let row = (side * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
            * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let staging = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("test tile readback"),
            size: (row * side) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &tiles.gpu.as_ref().expect("made").texture,
                mip_level: mip,
                origin: wgpu::Origin3d {
                    x: 0,
                    y: 0,
                    z: layer as u32,
                },
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &staging,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row),
                    rows_per_image: Some(side),
                },
            },
            wgpu::Extent3d {
                width: side,
                height: side,
                depth_or_array_layers: 1,
            },
        );
        gpu.queue.submit([encoder.finish()]);
        let (tx, rx) = std::sync::mpsc::channel();
        staging.slice(..).map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        gpu.device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("poll");
        rx.recv().unwrap().expect("map");
        let bytes = staging
            .slice(..)
            .get_mapped_range()
            .expect("mapped")
            .to_vec();
        staging.unmap();
        (0..side)
            .flat_map(|y| {
                let line = &bytes[(y * row) as usize..];
                (0..side as usize)
                    .map(|x| {
                        [
                            line[x * 4],
                            line[x * 4 + 1],
                            line[x * 4 + 2],
                            line[x * 4 + 3],
                        ]
                    })
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    /// A layer filled from an sRGB checker of 2 x 2 cells: its first level shows the cells,
    /// its last the checker's mean in linear light; a second texture in another layer leaves
    /// the first as it was.
    #[test]
    fn a_layer_holds_its_texture_down_to_its_mean() {
        let Some(gpu) = device() else {
            return;
        };
        let (light, dark) = (200u8, 50u8);
        let checker = |a: u8, b: u8| -> Vec<u8> {
            (0..64u32 * 64)
                .flat_map(|i| {
                    let (x, y) = (i % 64, i / 64);
                    let v = if (x / 32 + y / 32) % 2 == 0 { a } else { b };
                    [v, v, v, 255]
                })
                .collect()
        };
        let source = |pixels: &[u8]| {
            let texture = gpu.device.create_texture_with_data(
                &gpu.queue,
                &wgpu::TextureDescriptor {
                    label: Some("test checker"),
                    size: wgpu::Extent3d {
                        width: 64,
                        height: 64,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba8UnormSrgb,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                },
                wgpu::util::TextureDataOrder::LayerMajor,
                pixels,
            );
            texture.create_view(&Default::default())
        };
        let stand_in = gpu
            .device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("test stand-in"),
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
            .create_view(&wgpu::TextureViewDescriptor {
                dimension: Some(wgpu::TextureViewDimension::D2Array),
                ..Default::default()
            });
        let mut tiles = Tiles::new(stand_in);
        let views = [source(&checker(light, dark)), source(&checker(dark, dark))];
        // (Nearest first: the first into layer 0.)
        for (n, view) in views.iter().enumerate() {
            let asked = tiles.ask(name(n as u64), 1.0 + n as f32, 1, || {
                Some(RayTileSource {
                    view: view.clone(),
                    rect: [0.0, 0.0, 1.0, 1.0],
                    version: 0,
                    lasting: true,
                })
            });
            assert_eq!(asked, None, "filled at the frame's end");
        }
        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        assert!(
            tiles.encode(&gpu.device, &mut encoder, |_| 0, 1),
            "made now"
        );
        gpu.queue.submit([encoder.finish()]);
        let stats = tiles.stats();
        assert_eq!((stats.layers, stats.waiting, stats.filled), (2, 0, 2));
        // (22.37 MB.)
        assert!((stats.mb - 21.33).abs() < 0.01, "{} MiB", stats.mb);
        let first = read_level(&gpu, &tiles, 0, 0);
        let at = |x: usize, y: usize| first[y * SIZE as usize + x];
        // The cells, 64 texels each (two texels in from their edges: the bilinear upsample
        // blends the two texels at a cell's border).
        assert_eq!((at(10, 10)[0], at(100, 10)[0]), (light, dark));
        assert_eq!((at(10, 100)[0], at(100, 100)[0]), (dark, light));
        assert_eq!(at(10, 10)[3], 255);
        let mean = read_level(&gpu, &tiles, 0, MIPS - 1)[0];
        let want = (srgb_to_linear(light) + srgb_to_linear(dark)) * 0.5;
        let got = srgb_to_linear(mean[0]);
        assert!(
            (got / want - 1.0).abs() < 0.03,
            "the last level is the mean in linear light: {got:.4} for {want:.4} (stored {mean:?})"
        );
        let other = read_level(&gpu, &tiles, 1, MIPS - 1)[0];
        assert_eq!(other[0], dark, "the second texture's layer");
        assert_eq!(
            read_level(&gpu, &tiles, 0, 0)[10 * SIZE as usize + 10][0],
            light
        );
    }
}
