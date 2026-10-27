//! A meadow of real grass blades that burns with the fire field.
//!
//! [`GrassMeadow`] fills a rectangle with clumps of curved, tapered blades (a few hundred
//! thousand of them, from straw to olive, some with seed heads) wherever the ground's fuel is
//! grass: fuel patches of kind `None` (roads, fire pits) stay bare and soaked patches grow
//! lush and green. [`GrassFeature`] draws them on the GPU, one instance per blade, thinning
//! and simplifying them with distance, swaying in the wind and casting shadows.
//!
//! Blades stand on the ground (the `Ground` contract, sampled once per fire cell into a height
//! map the vertex shader reads), so a meadow can lie over a hillside.
//!
//! Each frame [`update_grass`] reads the [`FireField`] under the meadow into a [`GrassBurn`]
//! map (one texel per fire cell): standing grass stands; burning grass glows at its cut ends,
//! blackens and collapses within a second or so; burnt cells keep short black stubble with
//! ash at the tips; when the fuel regrows, green shoots push up through the stubble and grow
//! back to full height, then cure to straw over a minute or two.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use bytemuck::{Pod, Zeroable};
use engine_core::math::{DVec2, DVec3, Rng, Vec2, Vec3};
use engine_core::prelude::*;
use engine_render::shadows::{MAX_CASCADES, SKY_OCCLUSION_VIEW};
use engine_render::wgpu;
use engine_render::{
    FeatureStats, FrameContext, GpuShared, RenderFeature, RenderPhase, RenderSettings, ViewData,
};

use engine_world::Ground;

use crate::field::{Cell, CellState, FireField};
use crate::fuel::{FuelKind, FuelPatch};
use crate::sim::FirePatches;

pub const GRASS_COMMON: &str = include_str!("shaders/grass_common.wgsl");
pub const GRASS_SHADER: &str = include_str!("shaders/grass.wgsl");
pub const GRASS_SHADOW_SHADER: &str = include_str!("shaders/grass_shadow.wgsl");

/// A meadow of grass blades over the rectangle around its entity (local X/Z, no rotation),
/// standing on the ground (`Ground`; at the entity's height where there is none). Blades grow
/// only where the fire module's fuel is grass, and burn, collapse and regrow with the fire
/// field.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GrassMeadow {
    /// Half extents of the rectangle, meters.
    pub half_extents: [f32; 2],
    /// Blades per square meter near the camera (they thin out with distance).
    pub density: f32,
    /// Shortest and tallest blades, meters.
    pub height: [f32; 2],
    /// Blade width at the base, meters.
    pub width: f32,
    /// Share of blades that carry a seed head, 0..1.
    pub seed_heads: f32,
    /// How much the blades sway in the wind (1 = normal).
    pub sway: f32,
    pub seed: u32,
}

impl Default for GrassMeadow {
    fn default() -> Self {
        Self {
            half_extents: [10.0, 10.0],
            density: 140.0,
            height: [0.45, 1.0],
            width: 0.011,
            seed_heads: 0.06,
            sway: 1.0,
            seed: 1,
        }
    }
}

/// How the grass in one cell looks right now (each channel smoothed over time).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GrassCell {
    /// Standing height as a fraction of full height (stubble ≈ 0.12).
    pub height: f32,
    /// 0 fresh .. 1 black.
    pub char: f32,
    /// Glow of burning or smoldering blade ends, 0..1.
    pub glow: f32,
    /// Fresh green of regrowth, 0..1 (cures back to straw over time).
    pub green: f32,
}

impl Default for GrassCell {
    fn default() -> Self {
        Self {
            height: 1.0,
            char: 0.0,
            glow: 0.0,
            green: 0.0,
        }
    }
}

/// Stubble left by a fire, as a fraction of the grass's height.
pub const STUBBLE: f32 = 0.12;
/// Height regrowing shoots reach before the fuel comes back, as a fraction of full height.
const SHOOTS: f32 = 0.4;
/// Seconds for regrowth to cure from green to straw (time constant).
const CURE_SECS: f32 = 70.0;

/// What a fire cell asks of the grass on it. `None` when the cell carries no fuel.
pub fn grass_target(cell: Option<&Cell>) -> Option<GrassCell> {
    let Some(c) = cell else {
        return Some(GrassCell::default());
    };
    if c.load <= 0.0 || c.kind == FuelKind::None {
        return None;
    }
    let left = (c.fuel / c.load).clamp(0.0, 1.0);
    let collapsed = STUBBLE + (1.0 - STUBBLE) * left * left;
    Some(match c.state() {
        CellState::Intact | CellState::Bare => {
            // Preheated by a fire next door: blades wilt and brown before they catch.
            let wilt = engine_core::math::smoothstep(60.0, 260.0, c.temp);
            GrassCell {
                height: left * (1.0 - 0.25 * wilt),
                char: 0.35 * wilt,
                glow: 0.0,
                green: 0.0,
            }
        }
        CellState::Burning => GrassCell {
            height: collapsed,
            char: (1.0 - left).max(0.55 * c.burn).min(1.0),
            glow: c.burn.max(c.glow * 0.5).min(1.0),
            green: 0.0,
        },
        CellState::Smoldering => GrassCell {
            height: collapsed,
            char: 1.0,
            glow: c.glow.min(1.0) * 0.6,
            green: 0.0,
        },
        CellState::Burnt => {
            // Green shoots through the stubble over the second half of the wait to regrow.
            let shoots = if c.regrow_secs > 0.0 {
                engine_core::math::smoothstep(0.5, 1.0, c.burnt_for / c.regrow_secs)
            } else {
                0.0
            };
            GrassCell {
                height: collapsed.max(SHOOTS * shoots),
                char: 1.0 - shoots,
                glow: 0.0,
                green: shoots,
            }
        }
    })
}

impl GrassCell {
    /// Move toward `target` over `dt` seconds: burning grass collapses and blackens within a
    /// second, regrowth stands up over several seconds, and fresh green cures slowly.
    pub fn advance(&mut self, target: GrassCell, dt: f32) {
        let step = |v: &mut f32, t: f32, up: f32, down: f32| {
            let rate = if t > *v { up } else { down };
            *v += (t - *v) * (rate * dt).min(1.0);
        };
        step(&mut self.height, target.height, 0.5, 3.0);
        step(&mut self.char, target.char, 0.8, 4.0);
        step(&mut self.glow, target.glow, 8.0, 3.0);
        // Green rises with the shoots and fades as the new grass cures.
        if target.green > self.green {
            step(&mut self.green, target.green, 1.0, 1.0);
        } else {
            self.green = (self.green - dt / CURE_SECS * self.green.max(0.15)).max(target.green);
        }
    }

    fn texel(&self) -> [u8; 4] {
        let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
        [q(self.height), q(self.char), q(self.glow), q(self.green)]
    }
}

/// The burn state of a [`GrassMeadow`], one cell per fire-field cell (maintained by
/// [`update_grass`]; the renderer uploads it as a texture).
#[derive(Component, Debug, Clone, Default)]
pub struct GrassBurn {
    /// World X/Z of the map's minimum corner.
    pub min: DVec2,
    pub cell_m: f32,
    /// Cells along X and Z.
    pub size: [u32; 2],
    pub cells: Vec<GrassCell>,
    /// Bumped whenever the map changes visibly.
    pub version: u64,
    /// World height the map was made at (the meadow entity's).
    pub base_y: f64,
    /// Ground height at each cell's center above `base_y`, meters.
    pub ground: Vec<f32>,
    /// Different for every sampling of `ground`.
    pub ground_id: u64,
}

impl GrassBurn {
    fn texels(&self) -> Vec<u8> {
        self.cells.iter().flat_map(|c| c.texel()).collect()
    }
}

/// Read the fire field into every meadow's [`GrassBurn`] (Update). Replaces the old
/// per-entity grass: collapse while burning, stubble, regrowth.
/// A normal system (not `&mut World`), so it runs beside other modules' systems.
pub fn update_grass(
    mut commands: Commands,
    field: Option<Res<FireField>>,
    ground: Option<Res<Ground>>,
    time: Res<Time>,
    mut q: Query<(
        Entity,
        &GrassMeadow,
        &GlobalTransform,
        Option<&mut GrassBurn>,
    )>,
) {
    let Some(field) = field else {
        return;
    };
    static GROUND_IDS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let ground: Option<&Ground> = ground.as_deref();
    let dt = time.delta_secs();
    {
        let cell_m = field.cell_m;
        for (entity, meadow, gt, burn) in &mut q {
            let c = gt.translation();
            let half = Vec2::from(meadow.half_extents).as_dvec2();
            let min = DVec2::new(c.x, c.z) - half;
            let size = [
                ((half.x * 2.0) / cell_m as f64).ceil().max(1.0) as u32,
                ((half.y * 2.0) / cell_m as f64).ceil().max(1.0) as u32,
            ];
            let at = |i: u32| {
                let p = min
                    + (DVec2::new((i % size[0]) as f64, (i / size[0]) as f64) + 0.5)
                        * cell_m as f64;
                field.cell_at(DVec3::new(p.x, c.y, p.y))
            };
            match burn {
                Some(mut b)
                    if b.size == size && b.min == min && b.cell_m == cell_m && b.base_y == c.y =>
                {
                    let mut changed = false;
                    for (i, cell) in b.cells.iter_mut().enumerate() {
                        let Some(target) = grass_target(at(i as u32)) else {
                            continue;
                        };
                        let before = cell.texel();
                        cell.advance(target, dt);
                        changed |= cell.texel() != before;
                    }
                    if changed {
                        b.version += 1;
                    }
                }
                _ => {
                    // New (or resized) map: start at the targets, so a fire that was
                    // fast-forwarded shows its stubble straight away.
                    let cells = (0..size[0] * size[1])
                        .map(|i| grass_target(at(i)).unwrap_or_default())
                        .collect();
                    // Ask from above the highest ground a meadow this size might cover (for
                    // terrain with caves, the first surface below is the meadow's).
                    let hint = c.y + 2.0 + half.max_element();
                    // The ground is dear to ask (a planet's), so a row per task.
                    let mut heights = vec![0.0f32; (size[0] * size[1]) as usize];
                    let row = size[0] as usize;
                    engine_core::tasks::par_chunks_mut(&mut heights, row, |y, out| {
                        for (x, h) in out.iter_mut().enumerate() {
                            let p = min + (DVec2::new(x as f64, y as f64) + 0.5) * cell_m as f64;
                            *h = ground
                                .and_then(|g| g.height(DVec3::new(p.x, hint, p.y)))
                                .filter(|h| h.is_finite())
                                .map_or(0.0, |h| (h - c.y) as f32);
                        }
                    });
                    let ground = heights;
                    commands.entity(entity).insert(GrassBurn {
                        min,
                        cell_m,
                        size,
                        cells,
                        version: 1,
                        base_y: c.y,
                        ground,
                        ground_id: GROUND_IDS.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
                    });
                }
            }
        }
    }
}

// ------------------------------------------------------------------------------ blades

/// GPU layout of `Blade` in `grass_common.wgsl`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Pod, Zeroable)]
pub struct GpuBlade {
    pub pos: [f32; 2],
    pub a: u32,
    pub b: u32,
}

/// Blade flag: lush (wet ground) grass.
pub const BLADE_LUSH: u32 = 1;
/// Blade flag: a thin stem with a seed head.
pub const BLADE_SEED_HEAD: u32 = 2;

impl GpuBlade {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        pos: Vec2,
        height: f32,
        width: f32,
        bend: f32,
        yaw: f32,
        seed: f32,
        rank: f32,
        flags: u32,
    ) -> Self {
        let q16 = |v: f32| (v.clamp(0.0, 1.0) * 65535.0 + 0.5) as u32;
        let q8 = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u32;
        let yaw8 =
            ((yaw.rem_euclid(std::f32::consts::TAU) / std::f32::consts::TAU * 256.0) as u32) & 0xff;
        Self {
            pos: pos.to_array(),
            a: q16(height / 2.0) | q8(width / 0.04) << 16 | q8(bend) << 24,
            b: yaw8 | q8(seed) << 8 | q8(rank) << 16 | (flags & 0xff) << 24,
        }
    }

    /// Height in meters.
    pub fn height(&self) -> f32 {
        (self.a & 0xffff) as f32 / 65535.0 * 2.0
    }

    /// Density rank 0..1 (blades with a rank above the distance's density are dropped).
    pub fn rank(&self) -> f32 {
        ((self.b >> 16) & 0xff) as f32 / 255.0
    }

    pub fn flags(&self) -> u32 {
        self.b >> 24
    }
}

/// A square piece of the meadow whose blades are contiguous in the blade buffer, in random
/// order, so drawing a prefix thins it out evenly.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BladeChunk {
    /// X/Z from the meadow's center, meters.
    pub min: Vec2,
    pub max: Vec2,
    pub first: u32,
    pub count: u32,
    pub tallest: f32,
}

/// Side of a [`BladeChunk`], meters.
pub const CHUNK_M: f32 = 4.0;

/// What grows at a point of the meadow: nothing, dry grass, or lush grass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Growth {
    Bare,
    Dry,
    Lush,
}

/// Grow the blades of `meadow`. `growth` says what grows at a point (meadow-local X/Z).
pub fn grow_blades(
    meadow: &GrassMeadow,
    growth: &dyn Fn(Vec2) -> Growth,
) -> (Vec<GpuBlade>, Vec<BladeChunk>) {
    let half = Vec2::from(meadow.half_extents);
    let nx = ((half.x * 2.0) / CHUNK_M).ceil().max(1.0) as i32;
    let nz = ((half.y * 2.0) / CHUNK_M).ceil().max(1.0) as i32;
    let mut blades = Vec::new();
    let mut chunks = Vec::new();
    let per_clump = 9.0;
    let clump_spacing = (per_clump / meadow.density.max(1.0)).sqrt();
    let [h_lo, h_hi] = meadow.height;
    let noise = |p: Vec2, f: f32, s: u32| crate::textures::noise(p.x * f, p.y * f, 0, 0, s);
    for cz in 0..nz {
        for cx in 0..nx {
            let min = -half + Vec2::new(cx as f32, cz as f32) * CHUNK_M;
            let max = (min + Vec2::splat(CHUNK_M)).min(half);
            let mut rng = Rng::new(
                (meadow.seed as u64) << 32 ^ (cx as u64 & 0xffff) << 16 ^ (cz as u64 & 0xffff),
            );
            let first = blades.len();
            let mut tallest = 0.0f32;
            let gx = ((max.x - min.x) / clump_spacing).ceil() as i32;
            let gz = ((max.y - min.y) / clump_spacing).ceil() as i32;
            for iz in 0..gz {
                for ix in 0..gx {
                    // Jittered grid of clumps (tussocks), even cover without regular rows.
                    let c = min
                        + (Vec2::new(ix as f32, iz as f32) + Vec2::new(rng.f32(), rng.f32()))
                            * clump_spacing;
                    if c.x > max.x || c.y > max.y {
                        continue;
                    }
                    // Wavy, ragged edges where the grass meets bare ground.
                    let wobble = Vec2::new(
                        noise(c, 0.9, meadow.seed ^ 0x51),
                        noise(c, 0.9, meadow.seed ^ 0x52),
                    ) * 0.35;
                    let here = growth(c + wobble);
                    if here == Growth::Bare {
                        continue;
                    }
                    // Shorter, sparser grass right at an edge.
                    let mut edge = 0.0f32;
                    for d in [Vec2::X, -Vec2::X, Vec2::Y, -Vec2::Y] {
                        if growth(c + wobble + d * 0.45) == Growth::Bare {
                            edge += 0.25;
                        }
                    }
                    if rng.f32() < edge * 0.6 {
                        continue;
                    }
                    let lush = here == Growth::Lush;
                    // Patches of taller and shorter grass across the field.
                    let tall = 0.5
                        + 0.5 * noise(c, 0.13, meadow.seed ^ 0x77)
                        + 0.2 * noise(c, 0.6, meadow.seed ^ 0x78);
                    let clump_h = (h_lo
                        + (h_hi - h_lo) * tall.clamp(0.0, 1.0) * rng.range(0.7, 1.1))
                        * (1.0 - 0.45 * edge)
                        * if lush { 0.75 } else { 1.0 };
                    let seedy = !lush && rng.chance(meadow.seed_heads * 2.0);
                    let n = rng.range_i32(5, 14);
                    let radius = rng.range(0.025, 0.09);
                    let color = rng.f32();
                    for _ in 0..n {
                        let off = rng.in_unit_disk() * radius;
                        let p = c + off;
                        if p.x < -half.x || p.y < -half.y || p.x > half.x || p.y > half.y {
                            continue;
                        }
                        // Blades fan out of the clump: outer ones lean out farther.
                        let out = off.length() / radius.max(1e-3);
                        let yaw = if off.length() > 1e-4 {
                            off.y.atan2(off.x) + rng.range(-0.5, 0.5)
                        } else {
                            rng.range(0.0, std::f32::consts::TAU)
                        };
                        let head = seedy && rng.chance(0.5);
                        let h = clump_h
                            * rng.range(0.6, 1.1)
                            * if head { rng.range(1.1, 1.3) } else { 1.0 };
                        let bend =
                            (0.15 + 0.6 * out * rng.range(0.6, 1.2) + if head { 0.2 } else { 0.0 })
                                .min(1.0);
                        let seed = (color * 0.6 + rng.f32() * 0.4).fract();
                        let mut flags = 0;
                        if lush {
                            flags |= BLADE_LUSH;
                        }
                        if head {
                            flags |= BLADE_SEED_HEAD;
                        }
                        tallest = tallest.max(h);
                        blades.push(GpuBlade::new(
                            p,
                            h,
                            meadow.width * rng.range(0.7, 1.35),
                            bend,
                            yaw,
                            seed,
                            0.0,
                            flags,
                        ));
                    }
                }
            }
            // Shuffle so any prefix is an even thinning, then rank.
            let chunk = &mut blades[first..];
            for i in (1..chunk.len()).rev() {
                let j = (rng.next_u32() as usize) % (i + 1);
                chunk.swap(i, j);
            }
            let n = chunk.len();
            for (i, b) in chunk.iter_mut().enumerate() {
                let rank = (i as f32 + 0.5) / n.max(1) as f32;
                b.b = (b.b & !(0xff << 16)) | (((rank * 255.0) as u32).min(255) << 16);
            }
            if n > 0 {
                chunks.push(BladeChunk {
                    min,
                    max,
                    first: first as u32,
                    count: n as u32,
                    tallest,
                });
            }
        }
    }
    (blades, chunks)
}

/// Fraction of a chunk's blades drawn at distance `d` (matches `grass_density` in WGSL).
pub fn density_at(d: f32, full_m: f32, least: f32) -> f32 {
    (full_m / d.max(1e-3)).powf(1.5).clamp(least, 1.0)
}

// ------------------------------------------------------------------------------ rendering

/// GPU layout of `GrassParams` in `grass_common.wgsl`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct GpuGrassParams {
    origin: [f32; 4],
    map: [f32; 4],
    lod: [f32; 4],
    misc: [f32; 4],
}

/// Full density out to this distance, meters.
const FULL_DENSITY_M: f32 = 11.0;
/// Least fraction of blades kept far away.
const LEAST_DENSITY: f32 = 0.07;
/// Widest far blades get (×).
const WIDEST: f32 = 3.2;
/// Shadows from grass reach this far from the camera, meters.
const SHADOW_M: f32 = 32.0;

struct MeadowGpu {
    entity: Entity,
    fingerprint: u64,
    center: DVec3,
    chunks: Vec<BladeChunk>,
    blades: wgpu::Buffer,
    params: wgpu::Buffer,
    map_size: [u32; 2],
    map_texture: wgpu::Texture,
    map_version: u64,
    /// Ground heights under the meadow (R32Float, one texel per burn-map cell).
    ground_texture: wgpu::Texture,
    ground_id: u64,
    /// Lowest and highest ground under each chunk, meters above the center.
    chunk_y: Vec<[f32; 2]>,
    group: wgpu::BindGroup,
    blade_count: u32,
    seen: bool,
}

#[derive(Debug, Clone)]
struct GrassDraw {
    meadow: usize,
    vertices: std::ops::Range<u32>,
    instances: std::ops::Range<u32>,
}

/// Draws every [`GrassMeadow`] (added by `FirePlugin`).
#[derive(Default)]
pub struct GrassFeature {
    pipeline: Option<wgpu::RenderPipeline>,
    shadow_pipeline: Option<wgpu::RenderPipeline>,
    layout: Option<wgpu::BindGroupLayout>,
    sampler: Option<wgpu::Sampler>,
    meadows: Vec<MeadowGpu>,
    draws: Vec<GrassDraw>,
    shadow_draws: [Vec<GrassDraw>; MAX_CASCADES],
    stats: FeatureStats,
    /// The wind the blades sway in, smoothed over [`SWAY_SECS`]: the wind at the camera jumps
    /// with every local gust (a fire's updraft, walking about), and blades fed it straight
    /// jolted back and forth (Tobias's playtest, 2026-10-27).
    wind: Option<Vec3>,
}

/// Seconds over which the blades follow a change in the wind.
const SWAY_SECS: f32 = 2.0;

fn lod_vertices(band: u32) -> std::ops::Range<u32> {
    let segs = match band {
        0 => 7,
        1 => 4,
        _ => 2,
    };
    band * 64..band * 64 + 2 * (segs + 1)
}

/// What grows where, from the fire module's fuel patches (highest priority first).
fn growth_from_patches(
    patches: &[crate::fuel::PlacedPatch],
    center: DVec3,
) -> impl Fn(Vec2) -> Growth + '_ {
    move |p: Vec2| {
        let w = center + DVec3::new(p.x as f64, 0.0, p.y as f64);
        match patches.iter().find(|pp| pp.contains(w)) {
            Some(pp) if pp.patch.kind == FuelKind::None || pp.patch.load_kg_m2 <= 0.0 => {
                Growth::Bare
            }
            Some(pp) if pp.patch.moisture > 0.25 => Growth::Lush,
            _ => Growth::Dry,
        }
    }
}

impl GrassFeature {
    fn upload_meadow(
        &mut self,
        gpu: &GpuShared,
        entity: Entity,
        meadow: &GrassMeadow,
        center: DVec3,
        fingerprint: u64,
        patches: &[crate::fuel::PlacedPatch],
    ) -> Option<usize> {
        let layout = self.layout.as_ref()?;
        let sampler = self.sampler.as_ref()?;
        let growth = growth_from_patches(patches, center);
        let (blades, chunks) = grow_blades(meadow, &growth);
        let device = &gpu.device;
        let data: &[u8] = if blades.is_empty() {
            &[0u8; 16]
        } else {
            bytemuck::cast_slice(&blades)
        };
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("grass blades"),
            size: data.len() as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        gpu.queue.write_buffer(&buffer, 0, data);
        let params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("grass params"),
            size: std::mem::size_of::<GpuGrassParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        // A 1×1 "all standing" map until the burn map arrives.
        let map_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("grass burn map"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        gpu.queue.write_texture(
            map_texture.as_image_copy(),
            &GrassCell::default().texel(),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4),
                rows_per_image: Some(1),
            },
            wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
        );
        // Flat ground until the burn map (and its heights) arrives.
        let ground_texture = Self::ground_texture(gpu, [1, 1], &[0.0]);
        let group = Self::bind_group(
            device,
            layout,
            sampler,
            &buffer,
            &params,
            &map_texture,
            &ground_texture,
        );
        let gpu_meadow = MeadowGpu {
            entity,
            fingerprint,
            center,
            chunk_y: vec![[0.0; 2]; chunks.len()],
            chunks,
            blade_count: blades.len() as u32,
            blades: buffer,
            params,
            map_size: [1, 1],
            map_texture,
            map_version: 0,
            ground_texture,
            ground_id: 0,
            group,
            seen: true,
        };
        if let Some(i) = self.meadows.iter().position(|m| m.entity == entity) {
            self.meadows[i] = gpu_meadow;
            Some(i)
        } else {
            self.meadows.push(gpu_meadow);
            Some(self.meadows.len() - 1)
        }
    }

    fn ground_texture(gpu: &GpuShared, size: [u32; 2], heights: &[f32]) -> wgpu::Texture {
        let extent = wgpu::Extent3d {
            width: size[0],
            height: size[1],
            depth_or_array_layers: 1,
        };
        let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("grass ground heights"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R32Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        gpu.queue.write_texture(
            texture.as_image_copy(),
            bytemuck::cast_slice(heights),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(size[0] * 4),
                rows_per_image: Some(size[1]),
            },
            extent,
        );
        texture
    }

    fn bind_group(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        sampler: &wgpu::Sampler,
        blades: &wgpu::Buffer,
        params: &wgpu::Buffer,
        map: &wgpu::Texture,
        ground: &wgpu::Texture,
    ) -> wgpu::BindGroup {
        let view = map.create_view(&Default::default());
        let ground_view = ground.create_view(&Default::default());
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("grass"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: params.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: blades.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(&ground_view),
                },
            ],
        })
    }

    fn upload_map(&mut self, gpu: &GpuShared, i: usize, burn: &GrassBurn) {
        let (Some(layout), Some(sampler)) = (self.layout.as_ref(), self.sampler.as_ref()) else {
            return;
        };
        let m = &mut self.meadows[i];
        if m.map_version == burn.version && m.map_size == burn.size && m.ground_id == burn.ground_id
        {
            return;
        }
        let [w, h] = burn.size;
        let mut rebind = false;
        if m.ground_id != burn.ground_id && burn.ground.len() == (w * h) as usize {
            m.ground_texture = Self::ground_texture(gpu, burn.size, &burn.ground);
            m.ground_id = burn.ground_id;
            m.chunk_y = chunk_heights(&m.chunks, burn, m.center);
            rebind = true;
        }
        if m.map_size != burn.size {
            rebind = true;
            m.map_texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("grass burn map"),
                size: wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            m.map_size = burn.size;
        }
        if rebind {
            m.group = Self::bind_group(
                &gpu.device,
                layout,
                sampler,
                &m.blades,
                &m.params,
                &m.map_texture,
                &m.ground_texture,
            );
        }
        gpu.queue.write_texture(
            m.map_texture.as_image_copy(),
            &burn.texels(),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(w * 4),
                rows_per_image: Some(h),
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        m.map_version = burn.version;
    }
}

/// Lowest and highest ground (meters above `center`) under each chunk, from the burn map's
/// heights.
fn chunk_heights(chunks: &[BladeChunk], burn: &GrassBurn, center: DVec3) -> Vec<[f32; 2]> {
    let [w, h] = burn.size;
    let offset = (center.y - burn.base_y) as f32;
    let cell = burn.cell_m as f64;
    let texel = |x: f32, z: f32, round_up: bool| {
        let f = |v: f64, n: u32| {
            let t = v / cell - 0.5;
            let t = if round_up { t.ceil() } else { t.floor() };
            t.clamp(0.0, n as f64 - 1.0) as u32
        };
        (
            f(center.x + x as f64 - burn.min.x, w),
            f(center.z + z as f64 - burn.min.y, h),
        )
    };
    chunks
        .iter()
        .map(|c| {
            let (x0, z0) = texel(c.min.x, c.min.y, false);
            let (x1, z1) = texel(c.max.x, c.max.y, true);
            let mut range = [f32::INFINITY, f32::NEG_INFINITY];
            for z in z0..=z1 {
                for x in x0..=x1 {
                    let y = burn.ground[(z * w + x) as usize] - offset;
                    range = [range[0].min(y), range[1].max(y)];
                }
            }
            if range[0] > range[1] { [0.0; 2] } else { range }
        })
        .collect()
}

/// What the meadow's blades depend on. Patches count relative to the meadow, so a world shift
/// (which moves both) keeps the blades.
fn fingerprint(meadow: &GrassMeadow, center: DVec3, patches: &[crate::fuel::PlacedPatch]) -> u64 {
    let mut h = DefaultHasher::new();
    format!("{meadow:?}").hash(&mut h);
    for p in patches {
        // Millimeters, so float noise from the shift doesn't count.
        (p.center - center)
            .to_array()
            .map(|v| (v * 1000.0).round() as i64)
            .hash(&mut h);
        p.axis_x.to_array().map(f32::to_bits).hash(&mut h);
        format!("{:?}", p.patch).hash(&mut h);
    }
    h.finish()
}

impl RenderFeature for GrassFeature {
    fn name(&self) -> &str {
        "grass"
    }

    fn init(&mut self, gpu: &GpuShared) {
        let device = &gpu.device;
        let entry = |binding, ty, visibility| wgpu::BindGroupLayoutEntry {
            binding,
            visibility,
            ty,
            count: None,
        };
        let both = wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT;
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("grass"),
            entries: &[
                entry(
                    0,
                    wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    both,
                ),
                entry(
                    1,
                    wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    wgpu::ShaderStages::VERTEX,
                ),
                entry(
                    2,
                    wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    wgpu::ShaderStages::VERTEX,
                ),
                entry(
                    3,
                    wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    wgpu::ShaderStages::VERTEX,
                ),
                entry(
                    4,
                    wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    wgpu::ShaderStages::VERTEX,
                ),
            ],
        });
        let color = gpu.shader("grass", &format!("{GRASS_COMMON}\n{GRASS_SHADER}"), true);
        let shadow = gpu.shader(
            "grass shadow",
            &format!("{GRASS_COMMON}\n{GRASS_SHADOW_SHADER}"),
            false,
        );
        let make = |label: &str,
                    module: &wgpu::ShaderModule,
                    vs: &str,
                    fs: Option<&str>,
                    group0: &wgpu::BindGroupLayout,
                    shadow: bool| {
            let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(label),
                bind_group_layouts: &[Some(group0), Some(&layout)],
                immediate_size: 0,
            });
            let targets = [Some(wgpu::ColorTargetState {
                format: gpu.formats.hdr,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })];
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pl),
                vertex: wgpu::VertexState {
                    module,
                    entry_point: Some(vs),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleStrip,
                    cull_mode: None,
                    unclipped_depth: shadow
                        && gpu.features.contains(wgpu::Features::DEPTH_CLIP_CONTROL),
                    ..Default::default()
                },
                depth_stencil: Some(if shadow {
                    gpu.shadow_depth_state()
                } else {
                    gpu.depth_state(true)
                }),
                multisample: Default::default(),
                fragment: fs.map(|fs| wgpu::FragmentState {
                    module,
                    entry_point: Some(fs),
                    compilation_options: Default::default(),
                    targets: &targets,
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        self.pipeline = Some(make(
            "grass",
            &color,
            "vs_grass",
            Some("fs_grass"),
            &gpu.frame_layout,
            false,
        ));
        self.shadow_pipeline = Some(make(
            "grass shadow",
            &shadow,
            "vs_grass_shadow",
            None,
            &gpu.shadow_view_layout,
            true,
        ));
        self.sampler = Some(device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("grass burn map"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        }));
        self.layout = Some(layout);
    }

    fn prepare(
        &mut self,
        world: &mut World,
        gpu: &GpuShared,
        view: &ViewData,
        _settings: &RenderSettings,
    ) {
        self.draws.clear();
        for d in &mut self.shadow_draws {
            d.clear();
        }
        self.stats = FeatureStats::default();
        let flat = Vec3::new(view.wind.x, 0.0, view.wind.z);
        let k = 1.0 - (-view.delta.clamp(0.0, 0.25) / SWAY_SECS).exp();
        let wind = match self.wind {
            Some(w) => w + (flat - w) * k,
            None => flat,
        };
        self.wind = Some(wind);
        if self.layout.is_none() {
            return;
        }
        let mut q = world.query::<(Entity, &GrassMeadow, &GlobalTransform, Option<&GrassBurn>)>();
        let meadows: Vec<(Entity, GrassMeadow, DVec3, Option<GrassBurn>)> = q
            .iter(world)
            .map(|(e, m, gt, b)| (e, m.clone(), gt.translation(), b.cloned()))
            .collect();
        for m in &mut self.meadows {
            m.seen = false;
        }
        if meadows.is_empty() {
            self.meadows.clear();
            return;
        }
        // Wait until the fuel patches are indexed, so roads and fire pits stay bare.
        let patch_count = world.query::<&FuelPatch>().iter(world).count();
        let patches = world
            .get_resource::<FirePatches>()
            .map(|p| p.list.clone())
            .unwrap_or_default();
        if patches.len() != patch_count {
            return;
        }
        for (entity, meadow, center, burn) in meadows {
            let fp = fingerprint(&meadow, center, &patches);
            let existing = self.meadows.iter().position(|m| m.entity == entity);
            let index = match existing {
                Some(i) if self.meadows[i].fingerprint == fp => Some(i),
                _ => self.upload_meadow(gpu, entity, &meadow, center, fp, &patches),
            };
            let Some(i) = index else {
                continue;
            };
            if let Some(burn) = &burn {
                self.upload_map(gpu, i, burn);
            }
            let m = &mut self.meadows[i];
            m.seen = true;
            m.center = center;
            let rel = (m.center - view.origin).as_vec3();
            let (map_min, map_size) = match &burn {
                Some(b) => (
                    Vec2::new((b.min.x - m.center.x) as f32, (b.min.y - m.center.z) as f32),
                    Vec2::new(b.size[0] as f32 * b.cell_m, b.size[1] as f32 * b.cell_m),
                ),
                None => (
                    -Vec2::from(meadow.half_extents),
                    Vec2::from(meadow.half_extents) * 2.0,
                ),
            };
            let sun_up = view.sun_dir.y.clamp(0.0, 1.0);
            let params = GpuGrassParams {
                origin: [rel.x, rel.y, rel.z, 0.0],
                map: [map_min.x, map_min.y, map_size.x, map_size.y],
                lod: [FULL_DENSITY_M, LEAST_DENSITY, WIDEST, sun_up],
                misc: [0.0, meadow.sway, wind.x, wind.z],
            };
            gpu.queue
                .write_buffer(&m.params, 0, bytemuck::bytes_of(&params));
            // Cull and pick each chunk's density and detail.
            for (c, y) in m.chunks.iter().zip(&m.chunk_y) {
                let lo = rel + Vec3::new(c.min.x, y[0], c.min.y);
                let hi = rel + Vec3::new(c.max.x, y[1] + c.tallest + 0.3, c.max.y);
                let center = (lo + hi) * 0.5;
                let radius = (hi - lo).length() * 0.5;
                let nearest = Vec3::ZERO.clamp(lo, hi).length();
                let fraction = density_at(nearest, FULL_DENSITY_M, LEAST_DENSITY);
                let count = ((c.count as f32 * fraction).ceil() as u32 + 8).min(c.count);
                let band = if nearest < 9.0 {
                    0
                } else if nearest < 28.0 {
                    1
                } else {
                    2
                };
                let draw = GrassDraw {
                    meadow: i,
                    vertices: lod_vertices(band),
                    instances: c.first..c.first + count,
                };
                if view.frustum.intersects_sphere(center, radius) {
                    self.stats.instances += count;
                    self.stats.triangles += count as u64 * (draw.vertices.len() as u64 - 2);
                    self.draws.push(draw.clone());
                } else {
                    self.stats.culled += 1;
                }
                if view.sun_shadows && nearest < SHADOW_M {
                    let shadow = GrassDraw {
                        vertices: lod_vertices(band.max(1)),
                        ..draw
                    };
                    for k in 0..(view.cascades.count as usize).min(2) {
                        if view.cascades.sphere_in_cascade(k, center, radius) {
                            self.shadow_draws[k].push(shadow.clone());
                        }
                    }
                }
            }
        }
        self.meadows.retain(|m| m.seen);
        self.stats.draw_calls = self.draws.len() as u32;
        if let Some(mut d) = world.get_resource_mut::<Diagnostics>() {
            d.set("fire.grass_blades", self.stats.instances as f64);
        }
    }

    fn draw_shadow(&self, pass: &mut wgpu::RenderPass<'_>, cascade: usize, _ctx: &FrameContext) {
        if cascade >= MAX_CASCADES || cascade == SKY_OCCLUSION_VIEW {
            return;
        }
        let Some(p) = &self.shadow_pipeline else {
            return;
        };
        let draws = &self.shadow_draws[cascade];
        if draws.is_empty() {
            return;
        }
        pass.set_pipeline(p);
        let mut bound = usize::MAX;
        for d in draws {
            if d.meadow != bound {
                pass.set_bind_group(1, &self.meadows[d.meadow].group, &[]);
                bound = d.meadow;
            }
            pass.draw(d.vertices.clone(), d.instances.clone());
        }
    }

    fn draw(&self, phase: RenderPhase, pass: &mut wgpu::RenderPass<'_>, _ctx: &FrameContext) {
        if phase != RenderPhase::Opaque || self.draws.is_empty() {
            return;
        }
        let Some(p) = &self.pipeline else {
            return;
        };
        pass.set_pipeline(p);
        let mut bound = usize::MAX;
        for d in &self.draws {
            if d.meadow != bound {
                pass.set_bind_group(1, &self.meadows[d.meadow].group, &[]);
                bound = d.meadow;
            }
            pass.draw(d.vertices.clone(), d.instances.clone());
        }
    }

    fn stats(&self) -> FeatureStats {
        self.stats
    }

    fn describe(&self) -> serde_json::Value {
        serde_json::json!({
            "meadows": self.meadows.len(),
            "blades": self.meadows.iter().map(|m| m.blade_count).sum::<u32>(),
            "drawn": self.stats.instances,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fuel::CellFuel;
    use engine_render::{compose_shader, validate_wgsl};

    #[test]
    fn a_world_shift_keeps_the_blades() {
        let meadow = GrassMeadow::default();
        let patches = |offset: DVec3| {
            vec![crate::fuel::PlacedPatch {
                entity: Entity::PLACEHOLDER,
                center: DVec3::new(3.0, 0.0, -2.0) + offset,
                axis_x: Vec2::X,
                patch: FuelPatch::grass(crate::fuel::PatchShape::Disc { radius: 4.0 }),
            }]
        };
        let center = DVec3::new(1.0, 0.5, 2.0);
        let before = fingerprint(&meadow, center, &patches(DVec3::ZERO));
        let shift = DVec3::new(-1.0e5, 0.0, 2.5e4);
        assert_eq!(
            before,
            fingerprint(&meadow, center + shift, &patches(shift))
        );
        // Moving a patch within the meadow does change what grows.
        let moved = fingerprint(&meadow, center, &patches(DVec3::X));
        assert_ne!(before, moved);
    }

    fn grass_cell() -> Cell {
        Cell::from_fuel(
            CellFuel {
                kind: FuelKind::Grass,
                load_kg_m2: 0.4,
                moisture: 0.05,
                ignition_c: 300.0,
                regrow_secs: 100.0,
            },
            0.0,
        )
    }

    #[test]
    fn shaders_validate() {
        validate_wgsl(
            "grass",
            &compose_shader(&format!("{GRASS_COMMON}\n{GRASS_SHADER}"), true),
        )
        .unwrap();
        validate_wgsl(
            "grass shadow",
            &compose_shader(&format!("{GRASS_COMMON}\n{GRASS_SHADOW_SHADER}"), false),
        )
        .unwrap();
    }

    #[test]
    fn grass_state_follows_the_fire() {
        // Untouched and intact grass stands; bare ground has no grass.
        assert_eq!(grass_target(None), Some(GrassCell::default()));
        let mut c = grass_cell();
        assert_eq!(grass_target(Some(&c)).unwrap().height, 1.0);
        let mut bare = c;
        bare.kind = FuelKind::None;
        bare.load = 0.0;
        assert_eq!(grass_target(Some(&bare)), None);

        // Burning: it glows, blackens and collapses toward stubble as the fuel goes.
        c.burn = 0.9;
        c.fuel = 0.1;
        let burning = grass_target(Some(&c)).unwrap();
        assert!(burning.glow > 0.8, "{burning:?}");
        assert!(burning.char > 0.7, "{burning:?}");
        assert!(burning.height < 0.25, "{burning:?}");

        // Burnt out: black stubble, no glow.
        c.burn = 0.0;
        c.glow = 0.0;
        c.fuel = 0.0;
        let burnt = grass_target(Some(&c)).unwrap();
        assert_eq!(burnt.char, 1.0);
        assert_eq!(burnt.glow, 0.0);
        assert!((burnt.height - STUBBLE).abs() < 1e-6);

        // Late in the wait to regrow: green shoots through the stubble.
        c.burnt_for = 90.0;
        let shoots = grass_target(Some(&c)).unwrap();
        assert!(
            shoots.green > 0.5 && shoots.char < 0.5 && shoots.height > STUBBLE,
            "{shoots:?}"
        );

        // Regrown: full height again.
        c.fuel = c.load;
        c.burnt_for = 0.0;
        assert_eq!(grass_target(Some(&c)).unwrap().height, 1.0);
    }

    #[test]
    fn grass_collapses_fast_and_regrows_green_then_cures() {
        let mut g = GrassCell::default();
        let burning = GrassCell {
            height: STUBBLE,
            char: 1.0,
            glow: 1.0,
            green: 0.0,
        };
        for _ in 0..30 {
            g.advance(burning, 1.0 / 30.0);
        }
        // A second of fire: mostly collapsed and glowing, blackening.
        assert!(g.height < 0.2 && g.char > 0.5 && g.glow > 0.9, "{g:?}");
        for _ in 0..60 {
            g.advance(burning, 1.0 / 30.0);
        }
        // Two more: black.
        assert!(g.char > 0.9, "{g:?}");
        // Shoots, then the regrown grass: it grows back over several seconds, still green.
        let regrown = GrassCell::default();
        let shoots = GrassCell {
            height: SHOOTS,
            char: 0.0,
            glow: 0.0,
            green: 1.0,
        };
        for _ in 0..(30 * 5) {
            g.advance(shoots, 1.0 / 30.0);
        }
        assert!(g.green > 0.9 && g.char < 0.1, "{g:?}");
        for _ in 0..30 {
            g.advance(regrown, 1.0 / 30.0);
        }
        assert!(
            g.height > SHOOTS && g.height < 0.9,
            "grows gradually: {g:?}"
        );
        for _ in 0..(30 * 10) {
            g.advance(regrown, 1.0 / 30.0);
        }
        assert!(g.height > 0.97 && g.green > 0.5, "{g:?}");
        // A few minutes later it has cured to straw.
        for _ in 0..(30 * 240) {
            g.advance(regrown, 1.0 / 30.0);
        }
        assert!(g.green < 0.05, "{g:?}");
    }

    #[test]
    fn blades_fill_the_meadow_but_not_bare_ground() {
        let meadow = GrassMeadow {
            half_extents: [6.0, 4.0],
            density: 100.0,
            ..Default::default()
        };
        // A road along x = 2..3 and a wet strip at z > 2.
        let growth = |p: Vec2| {
            if (2.0..3.0).contains(&p.x) {
                Growth::Bare
            } else if p.y > 2.0 {
                Growth::Lush
            } else {
                Growth::Dry
            }
        };
        let (blades, chunks) = grow_blades(&meadow, &growth);
        let area = 12.0 * 8.0 - 8.0;
        let n = blades.len() as f32;
        assert!(n > area * 60.0 && n < area * 140.0, "{n} blades");
        // Nothing well inside the road; lush blades only in the wet strip.
        assert!(
            blades
                .iter()
                .all(|b| !(2.4..2.6).contains(&b.pos[0]) || b.pos[1].abs() > 10.0)
        );
        assert!(
            blades
                .iter()
                .filter(|b| b.flags() & BLADE_LUSH != 0)
                .all(|b| b.pos[1] > 1.4)
        );
        assert!(blades.iter().any(|b| b.flags() & BLADE_SEED_HEAD != 0));
        assert!(blades.iter().all(|b| b.height() <= 1.4));
        // Chunks cover the blades in order, and ranks spread evenly in each.
        let mut next = 0;
        for c in &chunks {
            assert_eq!(c.first, next);
            next += c.count;
            let slice = &blades[c.first as usize..(c.first + c.count) as usize];
            let mean = slice.iter().map(|b| b.rank()).sum::<f32>() / slice.len() as f32;
            assert!((mean - 0.5).abs() < 0.05, "{mean}");
            assert!(
                slice
                    .iter()
                    .all(|b| b.pos[0] >= c.min.x - 0.1 && b.pos[0] <= c.max.x + 0.1)
            );
        }
        assert_eq!(next as usize, blades.len());
        // Density falls off with distance but never below the floor.
        assert_eq!(density_at(5.0, FULL_DENSITY_M, LEAST_DENSITY), 1.0);
        assert!(density_at(40.0, FULL_DENSITY_M, LEAST_DENSITY) < 0.3);
        assert_eq!(
            density_at(500.0, FULL_DENSITY_M, LEAST_DENSITY),
            LEAST_DENSITY
        );
    }
}
