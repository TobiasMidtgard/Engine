//! Procedural PBR material library: ready-made surfaces (brick, rock, wood, rusty steel, …)
//! whose textures are generated on the GPU the way Substance Designer graphs build them.
//!
//! ```ignore
//! let wall = StandardMaterial::preset(MaterialPreset::Brick);
//! let cliff = StandardMaterial::preset(MaterialPreset::Rock).with_triplanar_world();
//! ```
//!
//! Every preset is a small WGSL graph (`shaders/materials/*.wgsl`) on top of a library of
//! tileable building blocks (`material_gen_lib.wgsl`: gradient, value and ridged noise, fbm,
//! domain warping, Worley cells with exact borders, scattered blobs, running-bond brick grids,
//! rounded boxes, scratches; slope blur and height reads for the refine pass in
//! `material_gen_main.wgsl`). Compute passes run it at a fixed physical size per tile:
//!
//! ```text
//! layers  (height in meters + masks: mortar, chips, rust, …)
//!   → refine (slope blur, erosion, water filling hollows; optional)
//!   → derive (height → tangent-space normal, ambient occlusion, curvature at two radii)
//!   → surface (base color, roughness, metalness from masks, curvature and occlusion:
//!              edge wear, dirt in cavities, color variation)
//!   → four RGBA8 maps with mip chains
//! ```
//!
//! | Map | Channels | Bound to |
//! |---|---|---|
//! | base color | RGB, sRGB | `base_color_texture` |
//! | normal | RGB tangent space, +Y up (glTF) | `normal_texture` |
//! | ORM | R occlusion, G roughness, B metalness | `metallic_roughness_texture` and `occlusion_texture` |
//! | height | R, 0 = deepest .. 1 = top | `height_texture` (parallax) |
//!
//! The maps are GPU-resident (they never exist on the CPU) and are built the first time a
//! material uses the preset, once per preset and resolution (`RenderSettings::
//! material_texture_size`, 1024² by default), then shared by every material and feature that
//! asks for them ([`GpuShared::material_textures`](crate::features::GpuShared::material_textures)).
//! [`GpuShared::read_material_maps`](crate::features::GpuShared::read_material_maps) copies them
//! back as [`Image`]s (tests, exporting to PNG).
//!
//! Heights and normals are physical: each preset is authored in millimeters for a tile of a
//! known size ([`MaterialPreset::tile_size`]), and its height map spans
//! [`MaterialPreset::relief_depth`] meters, which is also the parallax depth.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use bytemuck::{Pod, Zeroable};
use serde::{Deserialize, Serialize};
use wgpu::util::DeviceExt;

use crate::warmup::{Build, Taken, Warmup};

use crate::features::GpuShared;
use crate::renderer::layout_entry;
use crate::texture::Image;

/// A ready-made physically based surface. Use it with
/// [`StandardMaterial::preset`](crate::StandardMaterial::preset).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum MaterialPreset {
    /// Red brick wall: 215 × 65 mm bricks in running bond, raked mortar, chipped edges.
    Brick,
    /// Rounded granite setts in sandy joints with moss, polished on top by traffic.
    Cobblestone,
    /// Rough rock or cliff face: fractured planes, cracks, lichen and dirt in the crevices.
    Rock,
    /// Cast concrete: aggregate speckle, air holes, hairline cracks and stains.
    Concrete,
    /// Painted plaster wall: trowel marks, satin paint, a few cracks and chips.
    Plaster,
    /// Oiled wooden floor planks with grain, knots, butt joints and gaps.
    WoodPlanks,
    /// Furrowed tree bark (oak-like) with lichen in places.
    Bark,
    /// Steel plate with rust blooming from scratches, pitting and streaks.
    RustySteel,
    /// Painted steel panels with seams, rivets, chipped edges, scratches and grime.
    PaintedMetal,
    /// Wind-rippled sand.
    Sand,
    /// Forest soil and mud with pebbles, twigs and water in the hollows.
    ForestSoil,
    /// Loose angular gravel.
    Gravel,
    /// Wind-packed snow with drifts and sparkling grains.
    Snow,
    /// Polished white marble tiles with gray veins.
    Marble,
    /// Short grass over soil, seen from above.
    Grass,
}

/// How a preset is generated (physical size, relief and the shader that draws it).
struct PresetInfo {
    name: &'static str,
    source: &'static str,
    /// Meters per tile.
    tile: f32,
    /// Meters spanned by the height map.
    depth: f32,
    /// Ambient occlusion search radius, meters.
    ao_radius: f32,
    /// Radius of the fine curvature (edge wear), meters.
    curvature_radius: f32,
    /// Refine passes.
    refine: u32,
    seed: u32,
}

impl MaterialPreset {
    /// Every preset, in gallery order.
    pub const ALL: [MaterialPreset; 15] = [
        MaterialPreset::Brick,
        MaterialPreset::Cobblestone,
        MaterialPreset::Rock,
        MaterialPreset::Concrete,
        MaterialPreset::Plaster,
        MaterialPreset::WoodPlanks,
        MaterialPreset::Bark,
        MaterialPreset::RustySteel,
        MaterialPreset::PaintedMetal,
        MaterialPreset::Sand,
        MaterialPreset::ForestSoil,
        MaterialPreset::Gravel,
        MaterialPreset::Snow,
        MaterialPreset::Marble,
        MaterialPreset::Grass,
    ];

    fn info(self) -> PresetInfo {
        macro_rules! src {
            ($file:literal) => {
                include_str!(concat!("shaders/materials/", $file))
            };
        }
        let p = |name, source, tile, depth, ao_radius, curvature_radius, refine, seed| PresetInfo {
            name,
            source,
            tile,
            depth,
            ao_radius,
            curvature_radius,
            refine,
            seed,
        };
        match self {
            MaterialPreset::Brick => p("brick", src!("brick.wgsl"), 1.35, 0.016, 0.02, 0.002, 0, 1),
            MaterialPreset::Cobblestone => p(
                "cobblestone",
                src!("cobblestone.wgsl"),
                1.2,
                0.05,
                0.05,
                0.004,
                0,
                2,
            ),
            MaterialPreset::Rock => p("rock", src!("rock.wgsl"), 3.0, 0.3, 0.25, 0.012, 1, 3),
            MaterialPreset::Concrete => p(
                "concrete",
                src!("concrete.wgsl"),
                2.0,
                0.006,
                0.012,
                0.002,
                0,
                4,
            ),
            MaterialPreset::Plaster => p(
                "plaster",
                src!("plaster.wgsl"),
                2.0,
                0.004,
                0.01,
                0.002,
                0,
                5,
            ),
            MaterialPreset::WoodPlanks => p(
                "wood planks",
                src!("wood_planks.wgsl"),
                1.2,
                0.006,
                0.01,
                0.0015,
                0,
                6,
            ),
            MaterialPreset::Bark => p("bark", src!("bark.wgsl"), 1.0, 0.05, 0.03, 0.004, 0, 7),
            MaterialPreset::RustySteel => p(
                "rusty steel",
                src!("rusty_steel.wgsl"),
                1.0,
                0.003,
                0.005,
                0.001,
                0,
                8,
            ),
            MaterialPreset::PaintedMetal => p(
                "painted metal",
                src!("painted_metal.wgsl"),
                1.0,
                0.006,
                0.008,
                0.0015,
                1,
                9,
            ),
            MaterialPreset::Sand => p("sand", src!("sand.wgsl"), 2.0, 0.03, 0.05, 0.005, 0, 10),
            MaterialPreset::ForestSoil => p(
                "forest soil",
                src!("forest_soil.wgsl"),
                1.5,
                0.06,
                0.04,
                0.004,
                1,
                11,
            ),
            MaterialPreset::Gravel => p(
                "gravel",
                src!("gravel.wgsl"),
                1.0,
                0.035,
                0.03,
                0.003,
                0,
                12,
            ),
            MaterialPreset::Snow => p("snow", src!("snow.wgsl"), 3.0, 0.06, 0.1, 0.01, 0, 13),
            MaterialPreset::Marble => p(
                "marble",
                src!("marble.wgsl"),
                1.2,
                0.002,
                0.004,
                0.001,
                0,
                14,
            ),
            MaterialPreset::Grass => p("grass", src!("grass.wgsl"), 1.0, 0.04, 0.02, 0.002, 0, 15),
        }
    }

    /// Lowercase name ("brick", "wood planks").
    pub fn name(self) -> &'static str {
        self.info().name
    }

    /// The real-world size of one texture tile in meters (a 1.35 m square of brick wall holds
    /// 6 × 18 bricks). Materials made with [`StandardMaterial::preset`](crate::StandardMaterial::preset)
    /// start with this as their `texture_scale`.
    pub fn tile_size(self) -> f32 {
        self.info().tile
    }

    /// Depth of the relief in meters at [`tile_size`](Self::tile_size): the height map's 0..1
    /// range (mortar 9 mm behind the brick face, 0.3 m of rock relief). Used for parallax.
    pub fn relief_depth(self) -> f32 {
        self.info().depth
    }

    /// The mean diffuse albedo (linear RGB) of the generated maps: base color where the surface
    /// is not metal, times its ambient occlusion. Light bounced off a surface of this preset
    /// takes this color (probe GI's rays). `tests/materials.rs` checks it against the maps.
    pub fn mean_albedo(self) -> [f32; 3] {
        use MaterialPreset as P;
        match self {
            P::Brick => [0.26, 0.104, 0.07],
            P::Cobblestone => [0.113, 0.1, 0.088],
            P::Rock => [0.172, 0.148, 0.117],
            P::Concrete => [0.271, 0.259, 0.235],
            P::Plaster => [0.679, 0.623, 0.533],
            P::WoodPlanks => [0.257, 0.169, 0.107],
            P::Bark => [0.128, 0.098, 0.072],
            P::RustySteel => [0.103, 0.042, 0.026],
            P::PaintedMetal => [0.096, 0.123, 0.097],
            P::Sand => [0.581, 0.397, 0.197],
            P::ForestSoil => [0.047, 0.031, 0.019],
            P::Gravel => [0.172, 0.162, 0.15],
            P::Snow => [0.835, 0.852, 0.885],
            P::Marble => [0.734, 0.721, 0.697],
            P::Grass => [0.067, 0.102, 0.016],
        }
    }

    /// Whether some of the surface is bare metal (metalness 1).
    pub fn has_metal(self) -> bool {
        matches!(
            self,
            MaterialPreset::RustySteel | MaterialPreset::PaintedMetal
        )
    }

    /// Full WGSL of this preset's generator (validated by the crate's shader tests).
    pub fn generator_source(self) -> String {
        format!(
            "{}\n{}\n{}",
            include_str!("shaders/material_gen_lib.wgsl"),
            self.info().source,
            include_str!("shaders/material_gen_main.wgsl")
        )
    }
}

impl std::fmt::Display for MaterialPreset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

/// GPU-resident textures of one generated preset. Bound by the mesh feature like images; other
/// features can bind the views in their own groups.
pub struct MaterialTextures {
    pub preset: MaterialPreset,
    /// Texels per side of mip 0.
    pub size: u32,
    /// Unique per generated set (a regenerated set gets a new id).
    pub id: u64,
    /// Base color, sampled as sRGB.
    pub base_color: wgpu::TextureView,
    /// Tangent-space normal (+Y up), averaged but not renormalized in the mips.
    pub normal: wgpu::TextureView,
    /// R occlusion, G roughness, B metalness.
    pub orm: wgpu::TextureView,
    /// Height in R (0 deepest, 1 top).
    pub height: wgpu::TextureView,
    textures: [wgpu::Texture; 4],
    stats: wgpu::Buffer,
}

/// Maps of a preset copied back to the CPU (mip 0) by
/// [`GpuShared::read_material_maps`](crate::features::GpuShared::read_material_maps).
pub struct MaterialMaps {
    pub preset: MaterialPreset,
    /// sRGB base color.
    pub base_color: Image,
    /// Linear tangent-space normal.
    pub normal: Image,
    /// Linear R occlusion, G roughness, B metalness.
    pub orm: Image,
    /// Linear height in R.
    pub height: Image,
    /// Texels where the generator produced a NaN or infinity (0 for a healthy preset).
    pub non_finite_texels: u32,
    /// Texels whose base color left 0..1 before it was clamped.
    pub clamped_texels: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct GenParams {
    size: [u32; 4],
    dims: [f32; 4],
}

/// Compute pipelines of one preset.
struct PresetPipelines {
    layers: wgpu::ComputePipeline,
    refine: wgpu::ComputePipeline,
    derive: wgpu::ComputePipeline,
    surface: wgpu::ComputePipeline,
}

/// Layouts and pipelines shared by every preset.
struct Common {
    layers_layout: wgpu::BindGroupLayout,
    refine_layout: wgpu::BindGroupLayout,
    derive_layout: wgpu::BindGroupLayout,
    surface_layout: wgpu::BindGroupLayout,
    layers_pipeline_layout: wgpu::PipelineLayout,
    refine_pipeline_layout: wgpu::PipelineLayout,
    derive_pipeline_layout: wgpu::PipelineLayout,
    surface_pipeline_layout: wgpu::PipelineLayout,
    mip_layout: wgpu::BindGroupLayout,
    mip_linear: wgpu::ComputePipeline,
    mip_srgb: wgpu::ComputePipeline,
}

#[derive(Default)]
struct Inner {
    sets: HashMap<(MaterialPreset, u32), Arc<MaterialTextures>>,
    pipelines: HashMap<MaterialPreset, Arc<PresetPipelines>>,
    common: Option<Arc<Common>>,
    /// Every preset's pipelines compiling on worker threads (see [`MaterialLibrary::warm`]).
    warmup: Option<Warmup<MaterialPreset, Arc<PresetPipelines>>>,
    /// Presets whose pipelines were compiled when first used rather than warmed.
    built_inline: u32,
    next_id: u64,
}

/// Cache of generated material textures (one per renderer, in [`GpuShared`]).
#[derive(Default)]
pub struct MaterialLibrary {
    inner: Mutex<Inner>,
}

/// Map formats: everything is stored as RGBA8 (base color sRGB-encoded) so compute passes can
/// write it; base color is sampled through an sRGB view.
const MAP_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const WORK_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba32Float;
const BASE_COLOR: usize = 0;
const MAP_LABELS: [&str; 4] = [
    "material base color",
    "material normal",
    "material orm",
    "material height",
];

impl MaterialLibrary {
    /// The textures of `preset` at `size`² texels, generating them on first use.
    pub fn get(&self, gpu: &GpuShared, preset: MaterialPreset, size: u32) -> Arc<MaterialTextures> {
        let size = clamp_size(size);
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(set) = inner.sets.get(&(preset, size)) {
            return set.clone();
        }
        let started = std::time::Instant::now();
        let common = inner.common(gpu);
        let pipelines = inner.pipelines(gpu, &common, preset);
        let compiled_ms = started.elapsed().as_secs_f64() * 1000.0;
        inner.next_id += 1;
        let id = inner.next_id;
        let set = Arc::new(generate(gpu, &common, &pipelines, preset, size, id));
        gpu.side_work.preset(size);
        tracing::info!(
            "generated material preset '{}' at {size}x{size} in {:.0} ms ({compiled_ms:.0} ms \
             of it for its pipelines; GPU work queued)",
            preset.name(),
            started.elapsed().as_secs_f64() * 1000.0
        );
        inner.sets.insert((preset, size), set.clone());
        set
    }

    /// Compile every preset's generator pipelines on worker threads (after Unreal's PSO
    /// precaching): a preset's first use then only queues its GPU work, where compiling its
    /// shader took 70-600 ms on lavapipe (`RenderSettings::pipeline_warmup`).
    pub fn warm(&self, gpu: &GpuShared) {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if inner.warmup.is_some() {
            return;
        }
        let common = inner.common(gpu);
        let jobs: Vec<(MaterialPreset, Build<Arc<PresetPipelines>>)> = MaterialPreset::ALL
            .into_iter()
            .filter(|preset| !inner.pipelines.contains_key(preset))
            .map(|preset| {
                let (device, common) = (gpu.device.clone(), common.clone());
                let build: Build<Arc<PresetPipelines>> =
                    Box::new(move || Some(Arc::new(preset_pipelines(&device, &common, preset))));
                (preset, build)
            })
            .collect();
        let threads = std::thread::available_parallelism().map_or(2, |n| n.get() / 4);
        inner.warmup = Some(Warmup::start(
            "material pipelines",
            jobs,
            threads.clamp(1, 4),
        ));
    }

    /// Presets whose pipelines are still compiling, and presets whose pipelines a first use
    /// had to compile itself.
    pub fn warming(&self) -> (usize, u32) {
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        (
            inner.warmup.as_ref().map_or(0, Warmup::pending),
            inner.built_inline,
        )
    }

    /// Presets generated so far, with their sizes.
    pub fn generated(&self) -> Vec<(MaterialPreset, u32)> {
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let mut v: Vec<_> = inner.sets.keys().copied().collect();
        v.sort();
        v
    }

    /// Copy mip 0 of every map of `preset` back to the CPU (generating it first if needed).
    pub fn read_back(
        &self,
        gpu: &GpuShared,
        preset: MaterialPreset,
        size: u32,
    ) -> Result<MaterialMaps, String> {
        let set = self.get(gpu, preset, size);
        let device = &gpu.device;
        let size = set.size;
        let row = size * 4;
        let padded =
            row.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("material readback"),
        });
        let buffers: Vec<wgpu::Buffer> = set
            .textures
            .iter()
            .map(|texture| {
                let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("material readback"),
                    size: padded as u64 * size as u64,
                    usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                });
                encoder.copy_texture_to_buffer(
                    texture.as_image_copy(),
                    wgpu::TexelCopyBufferInfo {
                        buffer: &buffer,
                        layout: wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(padded),
                            rows_per_image: Some(size),
                        },
                    },
                    wgpu::Extent3d {
                        width: size,
                        height: size,
                        depth_or_array_layers: 1,
                    },
                );
                buffer
            })
            .collect();
        let stats = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("material stats readback"),
            size: 16,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        encoder.copy_buffer_to_buffer(&set.stats, 0, &stats, 0, 16);
        gpu.queue.submit([encoder.finish()]);

        let read = |buffer: &wgpu::Buffer| -> Result<Vec<u8>, String> {
            let (tx, rx) = std::sync::mpsc::channel();
            buffer.slice(..).map_async(wgpu::MapMode::Read, move |r| {
                let _ = tx.send(r);
            });
            device
                .poll(wgpu::PollType::wait_indefinitely())
                .map_err(|e| format!("GPU poll failed: {e}"))?;
            rx.recv()
                .map_err(|e| e.to_string())?
                .map_err(|e| format!("could not map the readback buffer: {e}"))?;
            let data = buffer
                .slice(..)
                .get_mapped_range()
                .map_err(|e| format!("could not read the readback buffer: {e:?}"))?
                .to_vec();
            buffer.unmap();
            Ok(data)
        };
        let mut images = Vec::with_capacity(4);
        for (i, buffer) in buffers.iter().enumerate() {
            let data = read(buffer)?;
            let mut pixels = Vec::with_capacity((row * size) as usize);
            for y in 0..size as usize {
                let start = y * padded as usize;
                pixels.extend_from_slice(&data[start..start + row as usize]);
            }
            images.push(Image::new(size, size, pixels, i == BASE_COLOR));
        }
        let counts = read(&stats)?;
        let count = |i: usize| {
            u32::from_le_bytes([
                counts[i * 4],
                counts[i * 4 + 1],
                counts[i * 4 + 2],
                counts[i * 4 + 3],
            ])
        };
        let mut images = images.into_iter();
        let mut next = || images.next().unwrap_or_default();
        Ok(MaterialMaps {
            preset,
            base_color: next(),
            normal: next(),
            orm: next(),
            height: next(),
            non_finite_texels: count(0),
            clamped_texels: count(1),
        })
    }
}

/// A power of two between 64 and 4096.
fn clamp_size(size: u32) -> u32 {
    size.clamp(64, 4096).next_power_of_two().min(4096)
}

impl Inner {
    fn common(&mut self, gpu: &GpuShared) -> Arc<Common> {
        if let Some(c) = &self.common {
            return c.clone();
        }
        let c = Arc::new(create_common(gpu));
        self.common = Some(c.clone());
        c
    }

    fn pipelines(
        &mut self,
        gpu: &GpuShared,
        common: &Common,
        preset: MaterialPreset,
    ) -> Arc<PresetPipelines> {
        if let Some(p) = self.pipelines.get(&preset) {
            return p.clone();
        }
        let p = match self.warmup.as_mut().map(|w| w.take(preset)) {
            Some(Taken::Ready(Some(p))) => p,
            _ => {
                self.built_inline += 1;
                Arc::new(preset_pipelines(&gpu.device, common, preset))
            }
        };
        self.pipelines.insert(preset, p.clone());
        p
    }
}

/// Compile `preset`'s generator shader and its pipelines (on any thread).
fn preset_pipelines(
    device: &wgpu::Device,
    common: &Common,
    preset: MaterialPreset,
) -> PresetPipelines {
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(&format!("material generator: {}", preset.name())),
        source: wgpu::ShaderSource::Wgsl(
            crate::features::compose_shader(&preset.generator_source(), false).into(),
        ),
    });
    let pipeline = |layout: &wgpu::PipelineLayout, entry: &str| {
        device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some(entry),
            layout: Some(layout),
            module: &module,
            entry_point: Some(entry),
            compilation_options: Default::default(),
            cache: None,
        })
    };
    PresetPipelines {
        layers: pipeline(&common.layers_pipeline_layout, "cs_layers"),
        refine: pipeline(&common.refine_pipeline_layout, "cs_refine"),
        derive: pipeline(&common.derive_pipeline_layout, "cs_derive"),
        surface: pipeline(&common.surface_pipeline_layout, "cs_surface"),
    }
}

fn create_common(gpu: &GpuShared) -> Common {
    use wgpu::ShaderStages as S;
    let device = &gpu.device;
    let uniform = wgpu::BindingType::Buffer {
        ty: wgpu::BufferBindingType::Uniform,
        has_dynamic_offset: false,
        min_binding_size: None,
    };
    let source = wgpu::BindingType::Texture {
        sample_type: wgpu::TextureSampleType::Float { filterable: false },
        view_dimension: wgpu::TextureViewDimension::D2,
        multisampled: false,
    };
    let storage = |format| wgpu::BindingType::StorageTexture {
        access: wgpu::StorageTextureAccess::WriteOnly,
        format,
        view_dimension: wgpu::TextureViewDimension::D2,
    };
    let stats = wgpu::BindingType::Buffer {
        ty: wgpu::BufferBindingType::Storage { read_only: false },
        has_dynamic_offset: false,
        min_binding_size: None,
    };
    let layout = |label: &str, entries: &[(u32, wgpu::BindingType)]| {
        let entries: Vec<_> = entries
            .iter()
            .map(|(b, ty)| layout_entry(*b, S::COMPUTE, *ty))
            .collect();
        device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some(label),
            entries: &entries,
        })
    };
    let layers_layout = layout(
        "material layers",
        &[
            (0, uniform),
            (5, storage(WORK_FORMAT)),
            (6, storage(WORK_FORMAT)),
        ],
    );
    let refine_layout = layout(
        "material refine",
        &[
            (0, uniform),
            (1, source),
            (2, source),
            (5, storage(WORK_FORMAT)),
            (6, storage(WORK_FORMAT)),
        ],
    );
    let derive_layout = layout(
        "material derive",
        &[
            (0, uniform),
            (1, source),
            (5, storage(WORK_FORMAT)),
            (6, storage(WORK_FORMAT)),
        ],
    );
    let surface_layout = layout(
        "material surface",
        &[
            (0, uniform),
            (1, source),
            (2, source),
            (3, source),
            (4, source),
            (7, storage(MAP_FORMAT)),
            (8, storage(MAP_FORMAT)),
            (9, storage(MAP_FORMAT)),
            (10, storage(MAP_FORMAT)),
            (11, stats),
        ],
    );
    let pipeline_layout = |l: &wgpu::BindGroupLayout| {
        device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("material generator"),
            bind_group_layouts: &[Some(l)],
            immediate_size: 0,
        })
    };
    let mip_layout = layout(
        "material mips",
        &[
            (
                0,
                wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
            ),
            (1, storage(MAP_FORMAT)),
        ],
    );
    let mip_pipeline_layout = pipeline_layout(&mip_layout);
    let mip_module = gpu.shader(
        "material mips",
        include_str!("shaders/material_gen_mips.wgsl"),
        false,
    );
    let mip = |entry: &str| {
        device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some(entry),
            layout: Some(&mip_pipeline_layout),
            module: &mip_module,
            entry_point: Some(entry),
            compilation_options: Default::default(),
            cache: None,
        })
    };
    Common {
        layers_pipeline_layout: pipeline_layout(&layers_layout),
        refine_pipeline_layout: pipeline_layout(&refine_layout),
        derive_pipeline_layout: pipeline_layout(&derive_layout),
        surface_pipeline_layout: pipeline_layout(&surface_layout),
        layers_layout,
        refine_layout,
        derive_layout,
        surface_layout,
        mip_linear: mip("cs_mip_linear"),
        mip_srgb: mip("cs_mip_srgb"),
        mip_layout,
    }
}

fn generate(
    gpu: &GpuShared,
    common: &Common,
    pipelines: &PresetPipelines,
    preset: MaterialPreset,
    size: u32,
    id: u64,
) -> MaterialTextures {
    use wgpu::TextureUsages as U;
    let device = &gpu.device;
    let info = preset.info();
    let extent = wgpu::Extent3d {
        width: size,
        height: size,
        depth_or_array_layers: 1,
    };
    let work = |label: &str| {
        device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: extent,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: WORK_FORMAT,
                usage: U::STORAGE_BINDING | U::TEXTURE_BINDING,
                view_formats: &[],
            })
            .create_view(&Default::default())
    };
    // Layers ping-pong between two pairs; derived values get their own pair.
    let layers = [
        [work("material layers a"), work("material layers b")],
        [work("material layers a'"), work("material layers b'")],
    ];
    let derived = [work("material derived a"), work("material derived b")];
    let mips = 32 - size.leading_zeros();
    let textures: [wgpu::Texture; 4] = std::array::from_fn(|i| {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some(MAP_LABELS[i]),
            size: extent,
            mip_level_count: mips,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: MAP_FORMAT,
            usage: U::STORAGE_BINDING | U::TEXTURE_BINDING | U::COPY_SRC,
            view_formats: if i == BASE_COLOR {
                &[wgpu::TextureFormat::Rgba8UnormSrgb]
            } else {
                &[]
            },
        })
    });
    let level = |t: &wgpu::Texture, l: u32| {
        t.create_view(&wgpu::TextureViewDescriptor {
            base_mip_level: l,
            mip_level_count: Some(1),
            ..Default::default()
        })
    };
    let stats = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("material stats"),
        contents: &[0u8; 16],
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    });
    let params = |iteration: u32| {
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("material generator params"),
            contents: bytemuck::bytes_of(&GenParams {
                size: [size, info.seed, iteration, 0],
                dims: [info.tile, info.depth, info.ao_radius, info.curvature_radius],
            }),
            usage: wgpu::BufferUsages::UNIFORM,
        })
    };
    let base_params = params(0);
    let group = |layout: &wgpu::BindGroupLayout, entries: &[(u32, wgpu::BindingResource)]| {
        let entries: Vec<_> = entries
            .iter()
            .map(|(binding, resource)| wgpu::BindGroupEntry {
                binding: *binding,
                resource: resource.clone(),
            })
            .collect();
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("material generator"),
            layout,
            entries: &entries,
        })
    };
    use wgpu::BindingResource as R;

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("material generator"),
    });
    let refine_params: Vec<wgpu::Buffer> = (0..info.refine).map(|i| params(i + 1)).collect();
    let mip_groups: Vec<(bool, wgpu::BindGroup, u32)> = textures
        .iter()
        .enumerate()
        .flat_map(|(i, t)| (1..mips).map(move |l| (i, t, l)))
        .map(|(i, t, l)| {
            let src = level(t, l - 1);
            let dst = level(t, l);
            (
                i == BASE_COLOR,
                group(
                    &common.mip_layout,
                    &[(0, R::TextureView(&src)), (1, R::TextureView(&dst))],
                ),
                (size >> l).max(1),
            )
        })
        .collect();
    {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("material generator"),
            timestamp_writes: None,
        });
        let mut run = |pipeline: &wgpu::ComputePipeline, group: &wgpu::BindGroup, n: u32| {
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, group, &[]);
            pass.dispatch_workgroups(n.div_ceil(8), n.div_ceil(8), 1);
        };
        let g = group(
            &common.layers_layout,
            &[
                (0, base_params.as_entire_binding()),
                (5, R::TextureView(&layers[0][0])),
                (6, R::TextureView(&layers[0][1])),
            ],
        );
        run(&pipelines.layers, &g, size);
        let mut current = 0;
        for p in &refine_params {
            let next = 1 - current;
            let g = group(
                &common.refine_layout,
                &[
                    (0, p.as_entire_binding()),
                    (1, R::TextureView(&layers[current][0])),
                    (2, R::TextureView(&layers[current][1])),
                    (5, R::TextureView(&layers[next][0])),
                    (6, R::TextureView(&layers[next][1])),
                ],
            );
            run(&pipelines.refine, &g, size);
            current = next;
        }
        let g = group(
            &common.derive_layout,
            &[
                (0, base_params.as_entire_binding()),
                (1, R::TextureView(&layers[current][0])),
                (5, R::TextureView(&derived[0])),
                (6, R::TextureView(&derived[1])),
            ],
        );
        run(&pipelines.derive, &g, size);
        let outputs: Vec<wgpu::TextureView> = textures.iter().map(|t| level(t, 0)).collect();
        let g = group(
            &common.surface_layout,
            &[
                (0, base_params.as_entire_binding()),
                (1, R::TextureView(&layers[current][0])),
                (2, R::TextureView(&layers[current][1])),
                (3, R::TextureView(&derived[0])),
                (4, R::TextureView(&derived[1])),
                (7, R::TextureView(&outputs[0])),
                (8, R::TextureView(&outputs[1])),
                (9, R::TextureView(&outputs[2])),
                (10, R::TextureView(&outputs[3])),
                (11, stats.as_entire_binding()),
            ],
        );
        run(&pipelines.surface, &g, size);
        for (srgb, g, n) in &mip_groups {
            let pipeline = if *srgb {
                &common.mip_srgb
            } else {
                &common.mip_linear
            };
            run(pipeline, g, *n);
        }
    }
    gpu.queue.submit([encoder.finish()]);

    let view = |t: &wgpu::Texture, format: Option<wgpu::TextureFormat>| {
        // Sampling views only: the sRGB reinterpretation cannot be a storage binding.
        t.create_view(&wgpu::TextureViewDescriptor {
            format,
            usage: Some(wgpu::TextureUsages::TEXTURE_BINDING),
            ..Default::default()
        })
    };
    MaterialTextures {
        preset,
        size,
        id,
        base_color: view(&textures[0], Some(wgpu::TextureFormat::Rgba8UnormSrgb)),
        normal: view(&textures[1], None),
        orm: view(&textures[2], None),
        height: view(&textures[3], None),
        textures,
        stats,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_have_unique_names_and_sane_sizes() {
        let mut names: Vec<_> = MaterialPreset::ALL.iter().map(|p| p.name()).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), MaterialPreset::ALL.len());
        for p in MaterialPreset::ALL {
            assert!(p.tile_size() > 0.2 && p.tile_size() < 10.0, "{p}");
            assert!(
                p.relief_depth() > 0.0 && p.relief_depth() < p.tile_size(),
                "{p}"
            );
        }
        assert_eq!(clamp_size(1000), 1024);
        assert_eq!(clamp_size(1), 64);
        assert_eq!(clamp_size(1 << 20), 4096);
    }

    #[test]
    fn preset_round_trips_through_json() {
        let v = serde_json::to_value(MaterialPreset::WoodPlanks).unwrap();
        assert_eq!(v, "WoodPlanks");
        let back: MaterialPreset = serde_json::from_value(v).unwrap();
        assert_eq!(back, MaterialPreset::WoodPlanks);
    }
}
