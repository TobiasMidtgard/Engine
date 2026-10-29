//! The water renderer: lakes and oceans on a camera-centered polar grid, flowing regions,
//! puddles and the underwater view. The light model is in `water.wgsl`.
//!
//! Lakes and oceans are drawn on one grid of rings around the camera whose spacing grows with
//! distance (fine near the camera, coarse at the horizon), so the cost is the same for a pond
//! and an ocean. The vertex shader displaces it with the body's Gerstner waves and the
//! interactive ripples; the fragment shader recomputes the waves per pixel down to what the
//! pixel can show. Everything draws in the transparent phase from the scene copies (refraction
//! and screen-space reflections).

use std::f64::consts::TAU;

use bytemuck::{Pod, Zeroable};
use engine_core::math::{DVec2, DVec3};
use engine_core::prelude::*;
use engine_render::features::{FeatureStats, FrameContext, GpuShared, RenderFeature, RenderPhase};
use engine_render::texture::f32_to_f16;
use engine_render::{RenderSettings, ViewData};
use engine_world::{Ground, SurfaceLayers, WeatherState};

use crate::body::{RainShower, WaterBed, WaterExtent, WaterOptics};
use crate::ripples::TILE;
use crate::state::{BodyState, SharedWater, WaterState};
use crate::waves::MAX_WAVES;

mod detail;
mod surface;

/// The water shader (composed after the renderer's `lib_types` and `lib_frame`).
pub const SHADER: &str = include_str!("water.wgsl");

/// Bodies drawn at once (the nearest ones).
pub const MAX_BODIES: usize = 16;
const MAX_SHOWERS: usize = 4;
/// Most plunge pools boiling under falls (nearest first).
const MAX_PLUNGES: usize = 8;
/// Plunge pools further than this from the camera don't boil, meters.
const PLUNGE_REACH: f64 = 400.0;
/// Grid points around each ring.
const SEGMENTS: u32 = 256;
/// Most rings a body can use (≈ 60 km at the finest spacing).
const MAX_RINGS: u32 = 420;
/// Puddle window, cells per side (of the ground-surface grid).
const PUDDLE_N: u32 = 128;
/// How far an ocean is drawn, meters.
const OCEAN_REACH: f64 = 30_000.0;
/// Segments down each falling curtain. Water falls free where its surface drops more than a
/// cell's width in one cell (steeper than 45°, [`crate::flow::FlowSim::fall_drop`]); the flow
/// surface hides triangles as steep as that (`fs_flow`).
const FALL_SEGMENTS: usize = crate::flow::FALL_POINTS;
/// `FallVertex::uv_speed.w` on a curtain where it slides down rock (0 where it falls free, 1
/// on a spray cloud): `fs_falls` reads anything under a half as how much it touches rock.
const ROCK_KIND: f32 = 0.45;
/// Most lips drawn (nearest first).
const MAX_LIPS: usize = 1024;
/// Lips further than this from the camera aren't drawn, meters.
const FALLS_REACH: f64 = 1500.0;

/// One vertex of a falling curtain (see `vs_falls`).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct FallVertex {
    /// Camera-relative position, and the water over the lip per meter of it (m²/s).
    rel_q: [f32; 4],
    /// Outward normal, and how far down the fall (0 lip, 1 foot).
    normal_frac: [f32; 4],
    /// Horizontal direction across the curtain, and which edge of this strip (-1, 1).
    across_side: [f32; 4],
    /// Meters across (continuous between neighboring strips), seconds since the water left
    /// the lip, speed it left at (m/s), and 0 (a curtain; 1 is a spray cloud, see
    /// [`mist_puff`]).
    uv_speed: [f32; 4],
    /// 1 on a strip's side where the fall ends (no neighboring lip carries the sheet on: its
    /// edge frays there), 0 inside it; then unused.
    edge: [f32; 4],
}

/// Turbulence of the water leaving a natural brink (rough rock, a ragged lip, a stream
/// already white above it): the share of its speed that wanders across. Laboratory spillways
/// run 1-5% (Castillo et al. 2015); a rough mountain brink several times that.
const FALL_TURBULENCE: f32 = 0.18;
/// `FallVertex::uv_speed.w` on the aerated spray round a falling curtain (see `fall_shell`).
const SHELL_KIND: f32 = -1.0;
/// `FallVertex::uv_speed.w` on the wet rock under a sheet sliding down it (see `fall_wet`).
const WET_KIND: f32 = -2.0;
/// How far back from its brink a curtain starts, meters: the water rolling over the edge
/// (glassy, bending down), so a fall doesn't begin at a ruled line.
const APPROACH_M: f32 = 0.35;

/// How far a free-falling jet of `q` m²/s that left its brink at `speed` m/s has spread to
/// each side of its core after falling `drop` meters: the turbulent spreading of a
/// rectangular jet, ξ = 2φ √h₀ (√H − √h₀) with φ = 1.07 Tu, h₀ the brink's velocity head
/// and H = h₀ + drop (Castillo, Carrillo & Sordo-Ward 2015); plus, once it has broken up
/// (past its break-up length L_b = 6 q^0.32, Horeni 1956), the spray thrown off its broken
/// clumps.
fn jet_spread(q: f32, speed: f32, drop: f32) -> f32 {
    let h0 = speed * speed / (2.0 * 9.81);
    let phi = 1.07 * FALL_TURBULENCE;
    let turbulent = 2.0 * phi * h0.sqrt() * ((h0 + drop.max(0.0)).sqrt() - h0.sqrt());
    let break_up = 6.0 * q.max(1e-4).powf(0.32);
    turbulent + 0.06 * (drop - 0.5 * break_up).max(0.0)
}

/// The curtain of water over `lip` as triangles down its `path` (see
/// [`FlowSim::fall_path`](crate::flow::FlowSim::fall_path)), relative to the camera via `rel`.
/// It starts a little upstream, rolling over the brink. Where it slides down rock it lies on
/// it, spreading wider, over a band of rock it wets darker (into `wet`, drawn first). Falling
/// free it is a volume: the glassy core, and in front of and behind it the aerated water and
/// spray that it spreads into as it falls ([`jet_spread`]), so it has depth from every side.
/// `sides`: whether neighboring lips carry the sheet on past each end (see
/// [`Falls::sides`](crate::flow::Falls::sides)); where none does, its edge frays.
fn fall_strip(
    lip: &crate::flow::Lip,
    path: &[crate::flow::FallPoint],
    sides: [bool; 2],
    rel: impl Fn(DVec3) -> Vec3,
    out: &mut Vec<FallVertex>,
    wet: &mut Vec<FallVertex>,
) {
    let across = Vec3::new(-lip.dir.y, 0.0, lip.dir.x);
    // Across coordinate from world x/z, so strips side by side share one pattern.
    let u0 = (lip.top.x * across.x as f64 + lip.top.z * across.z as f64).rem_euclid(4096.0) as f32;
    let last = path.len().saturating_sub(1).max(1);
    let mut spread = 0.0f32;
    // The water on its way to the brink, level with it and a little above the flow's own
    // surface there.
    let ahead = Vec3::new(lip.dir.x, 0.0, lip.dir.y);
    let approach = path.first().map(|p| crate::flow::FallPoint {
        pos: p.pos - (ahead * APPROACH_M).as_dvec3() + DVec3::Y * 0.015,
        vel: ahead * lip.speed.max(0.3),
        t: -APPROACH_M / lip.speed.max(0.3),
        rock: Vec3::ZERO,
    });
    // Core, front spray, back spray, wet rock.
    let mut prev: [Option<[FallVertex; 2]>; 4] = [None; 4];
    for (k, point) in approach.iter().chain(path).enumerate() {
        let frac = k.saturating_sub(1) as f32 / last as f32;
        let on_rock = point.rock != Vec3::ZERO;
        let normal = if on_rock {
            point.rock
        } else {
            across
                .cross(point.vel.normalize_or(Vec3::NEG_Y))
                .normalize_or(Vec3::Y)
        };
        // The sheet spreads and frays as it falls, and fans out a little over the rock it runs
        // down (neighboring lips' sheets overlap there, see `fs_falls`).
        if on_rock {
            spread += 0.06;
        }
        let half = lip.width * (0.66 + 0.3 * frac + spread.min(0.35));
        let drop = (lip.top.y - point.pos.y) as f32;
        let jet = jet_spread(lip.q, lip.speed, drop);
        // The core's own thickness: the water over the brink at the speed it falls.
        let core = (0.5 * lip.q / point.vel.length().max(0.3)).clamp(0.003, 0.3);
        let p = rel(point.pos);
        let layers = [
            (p, normal, half, 0.0, if on_rock { ROCK_KIND } else { 0.0 }),
            // On rock the spray lies over the sheet; falling free it surrounds the core.
            (
                p + normal * (core + jet),
                normal,
                half + jet,
                37.0,
                SHELL_KIND,
            ),
            (
                p - normal * if on_rock { 0.0 } else { core + jet },
                -normal,
                half + jet,
                71.0,
                SHELL_KIND,
            ),
            // The rock under the sheet, wet wider than the sheet runs (splashed, and where the
            // flow wandered). The strip lies at the sheet (the drawn rock is within a few
            // tenths of a meter of the flow's bed) and darkens the scene just behind it
            // (`fall_wet`).
            (
                p - normal * 0.01,
                normal,
                (half + jet) * 1.3 + 0.25,
                0.0,
                WET_KIND,
            ),
        ];
        for (layer, (at, n, w, du, kind)) in layers.into_iter().enumerate() {
            // Behind a sheet on rock is the rock; only there is rock to wet.
            if (layer == 2 && on_rock) || (layer == 3 && !on_rock) {
                prev[layer] = None;
                continue;
            }
            // (The spray is thrown off as the water falls: none before the brink.)
            if k == 0 && approach.is_some() && (layer == 1 || layer == 2) {
                continue;
            }
            let ring = [-1.0f32, 1.0].map(|side| {
                let open = !sides[(side > 0.0) as usize];
                FallVertex {
                    rel_q: (at + across * (w * side)).extend(lip.q).to_array(),
                    normal_frac: n.extend(frac).to_array(),
                    across_side: across.extend(side).to_array(),
                    uv_speed: [u0 + du + w * side, point.t, lip.speed, kind],
                    edge: [open as u8 as f32, 0.0, 0.0, 0.0],
                }
            });
            if let Some(before) = prev[layer] {
                // Wound so the front face is the one `normal` points out of.
                let to = if layer == 3 { &mut *wet } else { &mut *out };
                to.extend_from_slice(&[before[0], before[1], ring[0], before[1], ring[1], ring[0]]);
            }
            prev[layer] = Some(ring);
        }
    }
}

/// Radius of the white boil where a fall of `power` watts lands, meters: about 2 m under a
/// 5 m fall of 0.2 m³/s, growing with the root of flow times drop.
fn boil_radius(power: f32) -> f32 {
    (0.8 + 1.2 * (power / 9810.0).max(0.0).sqrt()).min(6.0)
}

/// The cloud of spray where a fall of `power` watts lands at `foot` (world; `rel` relative
/// to the camera): camera-facing quads (`right`, `up`) drawn over the curtain's foot, out the way it lands
/// (`away`, its horizontal velocity there). A dense
/// cloud boiling off the pool, and over it a plume that billows up, grows and thins (the
/// air the plunge drags down comes back up full of droplets). Corners go in
/// `normal_frac.xy`, the radius in `.z` and how dense it is in `.w`.
fn mist_puff(
    foot: DVec3,
    rel: Vec3,
    power: f32,
    away: Vec2,
    right: Vec3,
    up: Vec3,
    out: &mut Vec<FallVertex>,
) {
    // Flow times drop (m⁴/s), as for the spray particles: bigger falls make bigger clouds.
    let size = (power / 9810.0).max(0.0).sqrt();
    let radius = (0.8 + size * 0.9).min(8.0);
    let strength = (0.45 + size * 0.2).min(1.0);
    // Each cloud billows differently.
    let seed = (foot.x * 0.37 + foot.z * 0.61).rem_euclid(97.0) as f32;
    // (height over the foot, radius, density) in units of the foot cloud's radius: its lower
    // half is under the pool; the plume above it rises with the falls' power.
    let rise = (0.5 + size * 0.2).min(1.4);
    let outward = Vec3::new(away.x, 0.0, away.y).normalize_or_zero();
    let layers = [
        (0.1, 1.0, 1.0),
        (0.55 * rise, 1.3, 0.3),
        (1.1 * rise, 1.6, 0.12),
    ];
    for (k, (lift, scale, density)) in layers.into_iter().enumerate() {
        let r = radius * scale;
        // Out from the rock the water came down (where it lands against a wall the cloud
        // would be all but hidden in it) and up.
        let center = rel + outward * (radius * 0.6) + Vec3::Y * (radius * lift);
        let corner = |cx: f32, cy: f32| FallVertex {
            rel_q: (center + (right * cx + up * cy) * r).extend(0.0).to_array(),
            normal_frac: [cx, cy, r, strength * density],
            across_side: [0.0; 4],
            uv_speed: [seed + k as f32 * 13.7, 0.0, 0.0, 1.0],
            edge: [0.0; 4],
        };
        out.extend_from_slice(&[
            corner(-1.0, -1.0),
            corner(1.0, -1.0),
            corner(-1.0, 1.0),
            corner(-1.0, 1.0),
            corner(1.0, -1.0),
            corner(1.0, 1.0),
        ]);
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct GpuWave {
    a: [f32; 4],
    b: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct GpuBody {
    origin: [f32; 4],
    extent: [f32; 4],
    rot: [f32; 4],
    bed: [f32; 4],
    absorb: [f32; 4],
    scatter: [f32; 4],
    look: [f32; 4],
    grid: [f32; 4],
    grid2: [f32; 4],
    wind: [f32; 4],
    detail: [f32; 4],
    waves: [GpuWave; MAX_WAVES],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Globals {
    ripple: [f32; 4],
    rain: [f32; 4],
    showers: [[f32; 4]; MAX_SHOWERS],
    puddle: [f32; 4],
    puddle2: [f32; 4],
    counts: [u32; 4],
    under: [f32; 4],
    under2: [f32; 4],
    detail: [f32; 4],
    detail2: [f32; 4],
    plunges: [[f32; 4]; MAX_PLUNGES],
    coarse: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct WaterUniform {
    g: Globals,
    bodies: [GpuBody; MAX_BODIES],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct FlowUniform {
    origin: [f32; 4],
    dims: [f32; 4],
    absorb: [f32; 4],
    scatter: [f32; 4],
    look: [f32; 4],
}

fn optics_vectors(o: &WaterOptics) -> ([f32; 4], [f32; 4]) {
    (
        [
            o.absorption[0],
            o.absorption[1],
            o.absorption[2],
            o.scattering,
        ],
        // Per meter of scattering, what each band sends back (the shader multiplies by
        // the scattering).
        [
            o.scatter_color[0] * o.backscatter,
            o.scatter_color[1] * o.backscatter,
            o.scatter_color[2] * o.backscatter,
            o.roughness,
        ],
    )
}

/// The fine wind waves a body's wind sea raises, if any ([`crate::detail`]).
fn detail_sea(b: &BodyState) -> Option<crate::detail::DetailSea> {
    let w = &b.body.waves;
    let wind = engine_core::math::Vec2::new(b.wind.x, b.wind.z);
    let speed = wind.length();
    (w.wind_waves && speed > 0.2).then(|| crate::detail::DetailSea {
        wind: speed,
        dir: wind / speed,
        fetch_m: w.fetch_m,
        height_scale: w.height_scale,
        choppiness: w.choppiness,
    })
}

/// A body's view of the fine waves' tiles: (amplitude scale, cos and sin of the turn from the
/// tiles' wind to its own, the slope variance nothing draws).
fn body_detail(b: &BodyState, tiles: Option<&detail::DetailState>) -> [f32; 4] {
    // Slopes the Gerstner waves draw: (k·a)²/2 each.
    let gerstner: f32 = b
        .waves
        .waves
        .iter()
        .map(|w| 0.5 * (w.k * w.amp) * (w.k * w.amp))
        .sum();
    let sea = detail_sea(b);
    let wind = sea.map_or(0.0, |s| s.wind);
    let (scale, turn, held) = match (
        tiles,
        sea.and_then(|s| Some((s, crate::detail::Spectrum::of(&s)?))),
    ) {
        (Some(t), Some((s, spectrum))) => {
            let scale = t.scale_for(&spectrum);
            let turn = [t.sea.dir.dot(s.dir), t.sea.dir.perp_dot(s.dir)];
            (scale, turn, t.slope_variance * scale * scale)
        }
        _ => (0.0, [1.0, 0.0], 0.0),
    };
    [
        scale,
        turn[0],
        turn[1],
        crate::detail::residual_slope_variance(wind, gerstner + held),
    ]
}

/// One body's draw: which bands of the grid to draw.
#[derive(Debug, Clone, Copy)]
struct BodyDraw {
    first_band: u32,
    last_band: u32,
}

/// GPU resources of one flowing region.
struct FlowGpu {
    entity: Entity,
    nx: u32,
    nz: u32,
    texture: wgpu::Texture,
    uniform: wgpu::Buffer,
    index: wgpu::Buffer,
    index_count: u32,
    group: wgpu::BindGroup,
    version: u64,
    /// The bed the triangles were laid out for.
    bed_version: u64,
    drawn: bool,
}

struct Gpu {
    body: wgpu::RenderPipeline,
    flow: wgpu::RenderPipeline,
    puddle: wgpu::RenderPipeline,
    under: wgpu::RenderPipeline,
    group1_layout: wgpu::BindGroupLayout,
    group2_layout: wgpu::BindGroupLayout,
    uniform: wgpu::Buffer,
    grid_index: wgpu::Buffer,
    puddle_index: wgpu::Buffer,
    ripple: wgpu::Texture,
    /// The currents in the ripple grid: x, z (m/s), how fast the water turns (1/s).
    currents: wgpu::Texture,
    ripple_n: u32,
    ripple_version: u64,
    /// What the ripple textures hold (f16 texels), kept between uploads: tiles that were and
    /// stay still water aren't written again.
    ripple_data: Vec<u16>,
    ripple_currents: Vec<u16>,
    /// Per ripple tile, whether anything moved there at the last upload.
    ripple_live: Vec<bool>,
    /// Whether the last upload wrote the grid's own heights (only drawn while the dispersive
    /// surface isn't).
    ripple_heights: bool,
    puddle_tex: wgpu::Texture,
    group1: Option<wgpu::BindGroup>,
    flows: Vec<FlowGpu>,
    falls: wgpu::RenderPipeline,
    fall_buffer: wgpu::Buffer,
    /// Vertices the buffer holds, and how many to draw this frame.
    fall_capacity: usize,
    fall_count: u32,
    /// The fine wind waves' tiles.
    detail: detail::DetailGpu,
    /// The dispersive ripples over the ripple grid.
    surface: surface::SurfaceGpu,
}

/// What the puddle window holds.
#[derive(Default)]
struct PuddleWindow {
    /// World x/z of the corner of texel (0, 0).
    origin: DVec2,
    cell: f32,
    /// Ground height under each texel, world y, for the window `ground_at` names.
    ground: Vec<f64>,
    /// Where `ground` was read: (origin, cell, `WaterState::shifts`; a vertical shift keeps
    /// the origin). Read only while there are puddles to draw.
    ground_at: Option<(DVec2, f32, u64)>,
    /// The ground heights in the texture are relative to this world y.
    base: f64,
    /// Any standing water in the window.
    wet: bool,
    since_upload: f32,
}

/// The water [`RenderFeature`]. Added by [`crate::WaterPlugin`].
#[derive(Default)]
pub struct WaterFeature {
    gpu: Option<Gpu>,
    bodies: Vec<BodyDraw>,
    underwater: bool,
    puddles: PuddleWindow,
    stats: FeatureStats,
    showers: Option<QueryState<(&'static GlobalTransform, &'static RainShower)>>,
}

fn create_texture(
    device: &wgpu::Device,
    label: &str,
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: width.max(1),
            height: height.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    })
}

fn write_texture(
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    width: u32,
    height: u32,
    bytes_per_texel: u32,
    data: &[u8],
) {
    queue.write_texture(
        texture.as_image_copy(),
        data,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(width * bytes_per_texel),
            rows_per_image: Some(height),
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
}

/// Triangles of an `nx` × `nz` vertex grid (row-major, rows along +z), facing up.
///
/// With `heights` (one per vertex), each quad is split along the diagonal that follows them,
/// so a channel crossing the grid at an angle keeps straight banks.
fn grid_indices(nx: u32, nz: u32, heights: Option<&[f32]>) -> Vec<u32> {
    let mut out = Vec::with_capacity(((nx - 1) * (nz - 1) * 6) as usize);
    for j in 0..nz - 1 {
        for i in 0..nx - 1 {
            let a = j * nx + i;
            let b = a + 1;
            let c = a + nx;
            let d = c + 1;
            let along_ad = heights.is_some_and(|h| {
                let h = |v: u32| h[v as usize];
                (h(a) - h(d)).abs() < (h(b) - h(c)).abs()
            });
            if along_ad {
                out.extend_from_slice(&[a, c, d, a, d, b]);
            } else {
                out.extend_from_slice(&[a, c, b, b, c, d]);
            }
        }
    }
    out
}

/// Triangles of the polar grid: band `i` joins ring `i` to ring `i + 1`, `SEGMENTS * 6`
/// indices per band, facing up.
fn polar_indices() -> Vec<u32> {
    let s = SEGMENTS;
    let mut out = Vec::with_capacity(((MAX_RINGS - 1) * s * 6) as usize);
    for i in 0..MAX_RINGS - 1 {
        for j in 0..s {
            let a = i * s + j;
            let b = (i + 1) * s + j;
            let c = i * s + (j + 1) % s;
            let d = (i + 1) * s + (j + 1) % s;
            out.extend_from_slice(&[a, c, b, c, d, b]);
        }
    }
    out
}

/// Texels of a flow region: surface (m above its origin), depth, velocity x, z.
///
/// Cells under still water (lake reservoirs) are hidden: the lake draws them. A dry cell next
/// to water continues the water's surface when that surface is below its ground (so the
/// terrain cuts a smooth waterline), and drops under its ground otherwise (the foot of a
/// fall).
fn flow_texels(sim: &crate::flow::FlowSim) -> Vec<f32> {
    let (nx, nz) = (sim.nx, sim.nz);
    let wet = |idx: usize| sim.depth[idx] > 0.0015 && !sim.reservoir[idx].is_finite();
    let mut data = Vec::with_capacity(nx * nz * 4);
    for j in 0..nz {
        for i in 0..nx {
            let idx = j * nx + i;
            let bed = sim.bed[idx];
            if wet(idx) {
                let d = sim.depth[idx];
                // The shader works in world axes.
                let v = sim.world_dir(sim.vel[idx]);
                data.extend_from_slice(&[bed + d, d, v.x, v.y]);
                continue;
            }
            let mut best: Option<(f32, usize)> = None;
            if !sim.reservoir[idx].is_finite() {
                for (di, dj) in [
                    (-1i64, 0i64),
                    (1, 0),
                    (0, -1),
                    (0, 1),
                    (-1, -1),
                    (1, -1),
                    (-1, 1),
                    (1, 1),
                ] {
                    let (ii, jj) = (i as i64 + di, j as i64 + dj);
                    if ii < 0 || jj < 0 || ii >= nx as i64 || jj >= nz as i64 {
                        continue;
                    }
                    let n = jj as usize * nx + ii as usize;
                    if wet(n) {
                        let level = sim.bed[n] + sim.depth[n];
                        if best.is_none_or(|(b, _)| level > b) {
                            best = Some((level, n));
                        }
                    }
                }
            }
            match best {
                Some((level, n)) if level <= bed + 0.03 => {
                    let v = sim.world_dir(sim.vel[n]);
                    data.extend_from_slice(&[level, 0.006, v.x, v.y]);
                }
                // Under its own ground: the water's edge is where the surface dips below it.
                _ => data.extend_from_slice(&[bed - 0.08, 0.0, 0.0, 0.0]),
            }
        }
    }
    data
}

/// Radius of ring `i` for inner spacing `s0` and growth `alpha` (matches `vs_body`).
fn ring_radius(i: f64, s0: f64, alpha: f64) -> f64 {
    s0 * ((alpha * i).exp() - 1.0) / alpha
}

/// First ring at or beyond radius `r`.
fn ring_at(r: f64, s0: f64, alpha: f64) -> f64 {
    (1.0 + r.max(0.0) * alpha / s0).ln() / alpha
}

/// Nearest and farthest horizontal distance from `p` to the body's water, meters.
fn extent_range(b: &BodyState, p: DVec2) -> (f64, f64) {
    let c = DVec2::new(b.center.x, b.center.z);
    let d = (p - c).length();
    match b.body.extent {
        WaterExtent::Infinite => (0.0, OCEAN_REACH),
        WaterExtent::Circle { radius } => {
            let r = radius as f64;
            ((d - r).max(0.0), d + r)
        }
        WaterExtent::Rect { half_size } => {
            let local = b.local(DVec3::new(p.x, 0.0, p.y)).as_dvec2();
            let h = DVec2::new(half_size[0] as f64, half_size[1] as f64);
            let outside = (local.abs() - h).max(DVec2::ZERO);
            let far = (local.abs() + h).length();
            (outside.length(), far)
        }
    }
}

impl WaterFeature {
    fn build(gpu: &GpuShared) -> Gpu {
        let device = &gpu.device;
        // Shader work without a rebuild: `ENGINE_WATER_SHADER=<path to water.wgsl>` loads the
        // shader from that file instead of the copy built in.
        let source =
            std::env::var_os("ENGINE_WATER_SHADER").and_then(|p| std::fs::read_to_string(p).ok());
        let shader = gpu.shader("water", source.as_deref().unwrap_or(SHADER), true);
        let vf = wgpu::ShaderStages::VERTEX_FRAGMENT;
        let tex = |binding: u32, sample_type: wgpu::TextureSampleType| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: vf,
            ty: wgpu::BindingType::Texture {
                sample_type,
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let uniform_entry = wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: vf,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let group1_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("water"),
            entries: &[
                uniform_entry,
                tex(1, wgpu::TextureSampleType::Float { filterable: true }),
                tex(2, wgpu::TextureSampleType::Float { filterable: true }),
                tex(3, wgpu::TextureSampleType::Depth),
                tex(4, wgpu::TextureSampleType::Float { filterable: false }),
                tex(5, wgpu::TextureSampleType::Float { filterable: true }),
                wgpu::BindGroupLayoutEntry {
                    binding: 6,
                    visibility: vf,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                },
                tex(7, wgpu::TextureSampleType::Float { filterable: true }),
                tex(8, wgpu::TextureSampleType::Float { filterable: true }),
            ],
        });
        let group2_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("water flow"),
            entries: &[
                uniform_entry,
                tex(1, wgpu::TextureSampleType::Float { filterable: false }),
            ],
        });
        let layout1 = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("water"),
            bind_group_layouts: &[Some(&gpu.frame_layout), Some(&group1_layout)],
            immediate_size: 0,
        });
        let layout2 = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("water flow"),
            bind_group_layouts: &[
                Some(&gpu.frame_layout),
                Some(&group1_layout),
                Some(&group2_layout),
            ],
            immediate_size: 0,
        });
        let pipeline = |label: &str,
                        layout: &wgpu::PipelineLayout,
                        vs: &str,
                        fs: &str,
                        depth: wgpu::DepthStencilState| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some(vs),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    cull_mode: None,
                    ..Default::default()
                },
                depth_stencil: Some(depth),
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(fs),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: gpu.formats.hdr,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let always = wgpu::DepthStencilState {
            depth_compare: Some(wgpu::CompareFunction::Always),
            ..gpu.depth_state(false)
        };
        let body = pipeline(
            "water body",
            &layout1,
            "vs_body",
            "fs_body",
            gpu.depth_state(true),
        );
        let flow = pipeline(
            "water flow",
            &layout2,
            "vs_flow",
            "fs_flow",
            gpu.depth_state(true),
        );
        let puddle = pipeline(
            "water puddles",
            &layout1,
            "vs_puddle",
            "fs_puddle",
            gpu.depth_state(false),
        );
        let under = pipeline("underwater", &layout1, "vs_under", "fs_under", always);
        // Falling curtains: aerated water blended over everything (premultiplied alpha).
        let falls = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("water falls"),
            layout: Some(&layout1),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_falls"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<FallVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![
                        0 => Float32x4, 1 => Float32x4, 2 => Float32x4, 3 => Float32x4,
                        4 => Float32x4
                    ],
                })],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(gpu.depth_state(false)),
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_falls"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: gpu.formats.hdr,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let fall_capacity = 64 * FALL_SEGMENTS * 6;
        let fall_buffer = Self::fall_buffer(device, fall_capacity);

        use wgpu::util::DeviceExt;
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("water uniform"),
            size: std::mem::size_of::<WaterUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let grid_index = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("water grid"),
            contents: bytemuck::cast_slice(&polar_indices()),
            usage: wgpu::BufferUsages::INDEX,
        });
        let puddle_index = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("puddle grid"),
            contents: bytemuck::cast_slice(&grid_indices(PUDDLE_N, PUDDLE_N, None)),
            usage: wgpu::BufferUsages::INDEX,
        });
        let ripple = create_texture(
            device,
            "water ripples",
            8,
            8,
            wgpu::TextureFormat::Rgba16Float,
        );
        let currents = create_texture(
            device,
            "water currents",
            8,
            8,
            wgpu::TextureFormat::Rgba16Float,
        );
        let puddle_tex = create_texture(
            device,
            "puddles",
            PUDDLE_N,
            PUDDLE_N,
            wgpu::TextureFormat::Rg32Float,
        );
        let detail = detail::DetailGpu::new(gpu);
        let surface = surface::SurfaceGpu::new(gpu);
        Gpu {
            body,
            flow,
            puddle,
            under,
            group1_layout,
            group2_layout,
            uniform,
            grid_index,
            puddle_index,
            ripple,
            currents,
            ripple_n: 8,
            ripple_version: u64::MAX,
            ripple_data: Vec::new(),
            ripple_currents: Vec::new(),
            ripple_live: Vec::new(),
            ripple_heights: true,
            puddle_tex,
            group1: None,
            flows: Vec::new(),
            falls,
            fall_buffer,
            fall_capacity,
            fall_count: 0,
            detail,
            surface,
        }
    }

    fn fall_buffer(device: &wgpu::Device, vertices: usize) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("water falls"),
            size: (vertices * std::mem::size_of::<FallVertex>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    /// Curtains of water falling free over the flowing regions' lips.
    /// `plunges`: where the visible falls land (world), their horizontal velocity there
    /// (m/s) and their power (W).
    fn upload_falls(
        gpu: &mut Gpu,
        shared: &GpuShared,
        state: &WaterState,
        view: &ViewData,
        plunges: &[(DVec3, Vec2, f32)],
    ) {
        // The falls as the flow step last worked them out (not traced again each frame).
        type Drawn<'a> = (
            f64,
            &'a crate::flow::Lip,
            &'a [crate::flow::FallPoint],
            [bool; 2],
        );
        let mut lips: Vec<Drawn> =
            state
                .flows
                .iter()
                .filter(|sim| sim.visible)
                .flat_map(|sim| {
                    let falls = sim.falls();
                    falls.lips.iter().zip(&falls.paths).zip(&falls.sides).map(
                        |((lip, path), sides)| {
                            (lip.top.distance(view.origin), lip, path.as_slice(), *sides)
                        },
                    )
                })
                .filter(|(d, ..)| *d < FALLS_REACH)
                .collect();
        lips.sort_by(|a, b| a.0.total_cmp(&b.0));
        lips.truncate(MAX_LIPS);
        let mut vertices = Vec::with_capacity(lips.len() * crate::flow::FALL_POINTS * 24);
        let mut curtains = Vec::with_capacity(lips.len() * crate::flow::FALL_POINTS * 18);
        for (_, lip, path, sides) in &lips {
            fall_strip(
                lip,
                path,
                *sides,
                |p| view.relative(p),
                &mut curtains,
                &mut vertices,
            );
        }
        // The wet rock first, under the curtains; spray clouds last, over their feet.
        vertices.append(&mut curtains);
        if !lips.is_empty() {
            let (right, up) = (view.rotation * Vec3::X, view.rotation * Vec3::Y);
            for &(foot, away, power) in plunges {
                if foot.distance(view.origin) < FALLS_REACH {
                    let rel = view.relative(foot);
                    mist_puff(foot, rel, power, away, right, up, &mut vertices);
                }
            }
        }
        gpu.fall_count = vertices.len() as u32;
        if vertices.is_empty() {
            return;
        }
        if vertices.len() > gpu.fall_capacity {
            gpu.fall_capacity = vertices.len().next_power_of_two();
            gpu.fall_buffer = Self::fall_buffer(&shared.device, gpu.fall_capacity);
        }
        shared
            .queue
            .write_buffer(&gpu.fall_buffer, 0, bytemuck::cast_slice(&vertices));
    }

    /// Fill one body's uniform and decide which grid bands to draw.
    fn body_uniform(b: &BodyState, view: &ViewData) -> Option<(GpuBody, BodyDraw)> {
        let cam = view.origin;
        let cam_xz = DVec2::new(cam.x, cam.z);
        let (near, far) = extent_range(b, cam_xz);
        let level = b.center.y - cam.y;
        let height = level.abs() + 0.3;
        let spread = b.waves.waves.iter().map(|w| w.horiz).sum::<f32>() as f64 + 1.0;
        // Fine near the camera, coarser when looking down from high up.
        let s0 = (height * 0.025).clamp(0.04, 8.0);
        let alpha = TAU / SEGMENTS as f64;
        let first = ring_at(near - spread, s0, alpha).floor().max(0.0) as u32;
        let last = (ring_at(far + spread, s0, alpha).ceil() as u32 + 1).min(MAX_RINGS - 1);
        if first >= last {
            return None;
        }
        let r_max = ring_radius(last as f64, s0, alpha);
        let rel = |x: f64, z: f64| [(x - cam.x) as f32, (z - cam.z) as f32];
        let center = rel(b.center.x, b.center.z);
        let (kind, ext) = match b.body.extent {
            WaterExtent::Infinite => (0.0, [0.0, 0.0]),
            WaterExtent::Circle { radius } => (1.0, [radius, 0.0]),
            WaterExtent::Rect { half_size } => (2.0, half_size),
        };
        let (bed_kind, bed_depth, shore) = match b.body.bed {
            WaterBed::Ground => (0.0, b.mean_depth, 0.0),
            WaterBed::Flat { depth_m } => (1.0, depth_m, 0.0),
            WaterBed::Bowl { depth_m, shore_m } => (2.0, depth_m, shore_m),
        };
        let mut waves = [GpuWave::zeroed(); MAX_WAVES];
        // Waves are referenced to the camera (exactly, in f64) so positions stay small.
        let shift = cam_xz - DVec2::new(b.center.x, b.center.z);
        for (g, w) in waves.iter_mut().zip(&b.waves.waves) {
            let phase = (w.phase as f64 - w.k as f64 * w.dir.as_dvec2().dot(shift)).rem_euclid(TAU);
            g.a = [w.dir.x, w.dir.y, w.k, w.amp];
            g.b = [w.horiz, phase as f32, w.omega, 0.0];
        }
        let wind = b.wind;
        let speed = (wind.x * wind.x + wind.z * wind.z).sqrt();
        let dir = if speed > 1e-3 {
            [wind.x / speed, wind.z / speed]
        } else {
            [1.0, 0.0]
        };
        let (absorb, scatter) = optics_vectors(&b.body.optics);
        let o = &b.body.optics;
        let (sin, cos) = b.yaw.sin_cos();
        let gpu = GpuBody {
            origin: [0.0, level as f32, 0.0, kind],
            extent: [center[0], center[1], ext[0], ext[1]],
            rot: [cos, sin, bed_kind, bed_depth],
            bed: [
                shore,
                b.mean_depth,
                speed,
                b.waves.waves.len().min(MAX_WAVES) as f32,
            ],
            absorb,
            scatter,
            look: [
                o.foam,
                o.caustics,
                o.refraction,
                if b.body.interactive { 1.0 } else { 0.0 },
            ],
            grid: [0.0, 0.0, s0 as f32, alpha as f32],
            grid2: [
                (last + 1) as f32,
                SEGMENTS as f32,
                r_max as f32,
                b.waves.max_height().max(0.01),
            ],
            wind: [dir[0], dir[1], b.body.current[0], b.body.current[1]],
            // Filled in by `prepare` once the fine waves' tiles are known.
            detail: [0.0; 4],
            waves,
        };
        Some((
            gpu,
            BodyDraw {
                first_band: first,
                last_band: last,
            },
        ))
    }

    /// `heights`: write the grid's own heights and slopes too (the dispersive surface's
    /// replace them while it is drawn; only the foam is read then).
    fn upload_ripples(gpu: &mut Gpu, shared: &GpuShared, state: &WaterState, heights: bool) {
        let r = &state.ripples;
        let n = r.n as u32;
        if gpu.ripple_n != n {
            gpu.ripple = create_texture(
                &shared.device,
                "water ripples",
                n,
                n,
                wgpu::TextureFormat::Rgba16Float,
            );
            gpu.currents = create_texture(
                &shared.device,
                "water currents",
                n,
                n,
                wgpu::TextureFormat::Rgba16Float,
            );
            gpu.ripple_n = n;
            gpu.ripple_version = u64::MAX;
            gpu.group1 = None;
        }
        if gpu.ripple_heights != heights {
            gpu.ripple_heights = heights;
            gpu.ripple_version = u64::MAX;
            gpu.ripple_live.fill(true);
        }
        if gpu.ripple_version == r.version {
            return;
        }
        gpu.ripple_version = r.version;
        let n = r.n;
        let inv = 0.5 / r.cell;
        let t = n.div_ceil(TILE);
        if gpu.ripple_data.len() != n * n * 4 {
            gpu.ripple_data = vec![0; n * n * 4];
            gpu.ripple_currents = vec![0; n * n * 4];
            gpu.ripple_live = vec![true; t * t];
        }
        // Tiles where anything moves now; with those that did at the last upload and their
        // neighbors (slopes reach across a tile's edge), the only ones whose texels change.
        let live: Vec<bool> = (0..t * t)
            .map(|k| {
                let (tx, ty) = (k % t, k / t);
                (ty * TILE..((ty + 1) * TILE).min(n)).any(|j| {
                    let row = j * n + tx * TILE..j * n + ((tx + 1) * TILE).min(n);
                    r.h[row.clone()].iter().any(|v| *v != 0.0)
                        || r.foam[row.clone()].iter().any(|v| *v != 0.0)
                        || r.bubbles[row.clone()].iter().any(|v| *v != 0.0)
                        || r.vel[row].iter().any(|v| *v != [0.0; 2])
                })
            })
            .collect();
        let near = |flags: &[bool], tx: usize, ty: usize| {
            (ty.saturating_sub(1)..=(ty + 1).min(t - 1))
                .any(|y| (tx.saturating_sub(1)..=(tx + 1).min(t - 1)).any(|x| flags[y * t + x]))
        };
        let (data, currents) = (&mut gpu.ripple_data, &mut gpu.ripple_currents);
        for ty in 0..t {
            for tx in 0..t {
                if !near(&live, tx, ty) && !near(&gpu.ripple_live, tx, ty) {
                    continue;
                }
                for j in ty * TILE..((ty + 1) * TILE).min(n) {
                    for i in tx * TILE..((tx + 1) * TILE).min(n) {
                        let idx = j * n + i;
                        let o = idx * 4;
                        if heights {
                            let h = |ii: usize, jj: usize| r.h[jj * n + ii];
                            let gx = (h((i + 1).min(n - 1), j) - h(i.saturating_sub(1), j)) * inv;
                            let gz = (h(i, (j + 1).min(n - 1)) - h(i, j.saturating_sub(1))) * inv;
                            data[o] = f32_to_f16(r.h[idx]);
                            data[o + 2] = f32_to_f16(gx);
                            data[o + 3] = f32_to_f16(gz);
                        }
                        data[o + 1] = f32_to_f16(r.foam[idx]);
                        let v = r.cell_current(i, j);
                        currents[o] = f32_to_f16(v.x);
                        currents[o + 1] = f32_to_f16(v.y);
                        currents[o + 3] = f32_to_f16(r.bubbles[idx]);
                    }
                }
            }
        }
        gpu.ripple_live = live;
        let (data, currents) = (&gpu.ripple_data, &gpu.ripple_currents);
        write_texture(
            &shared.queue,
            &gpu.ripple,
            n as u32,
            n as u32,
            8,
            bytemuck::cast_slice(data.as_slice()),
        );
        write_texture(
            &shared.queue,
            &gpu.currents,
            n as u32,
            n as u32,
            8,
            bytemuck::cast_slice(currents.as_slice()),
        );
    }

    fn upload_flows(gpu: &mut Gpu, shared: &GpuShared, state: &WaterState, view: &ViewData) {
        let device = &shared.device;
        gpu.flows.retain(|f| {
            state.flows.iter().any(|s| {
                s.entity == f.entity
                    && s.nx as u32 == f.nx
                    && s.nz as u32 == f.nz
                    && s.bed_version == f.bed_version
            })
        });
        for f in &mut gpu.flows {
            f.drawn = false;
        }
        for sim in &state.flows {
            if !sim.visible {
                continue;
            }
            let pos = match gpu.flows.iter().position(|f| f.entity == sim.entity) {
                Some(p) => p,
                None => {
                    use wgpu::util::DeviceExt;
                    let (nx, nz) = (sim.nx as u32, sim.nz as u32);
                    let texture = create_texture(
                        device,
                        "water flow",
                        nx,
                        nz,
                        wgpu::TextureFormat::Rgba32Float,
                    );
                    let uniform = device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some("water flow"),
                        size: std::mem::size_of::<FlowUniform>() as u64,
                        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                        mapped_at_creation: false,
                    });
                    let indices = grid_indices(nx, nz, Some(&sim.bed));
                    let index = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("water flow grid"),
                        contents: bytemuck::cast_slice(&indices),
                        usage: wgpu::BufferUsages::INDEX,
                    });
                    let tex_view = texture.create_view(&Default::default());
                    let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("water flow"),
                        layout: &gpu.group2_layout,
                        entries: &[
                            wgpu::BindGroupEntry {
                                binding: 0,
                                resource: uniform.as_entire_binding(),
                            },
                            wgpu::BindGroupEntry {
                                binding: 1,
                                resource: wgpu::BindingResource::TextureView(&tex_view),
                            },
                        ],
                    });
                    gpu.flows.push(FlowGpu {
                        entity: sim.entity,
                        nx,
                        nz,
                        texture,
                        uniform,
                        index,
                        index_count: indices.len() as u32,
                        group,
                        version: u64::MAX,
                        bed_version: sim.bed_version,
                        drawn: false,
                    });
                    gpu.flows.len() - 1
                }
            };
            let f = &mut gpu.flows[pos];
            if !sim.depth.iter().any(|d| *d > 0.003) {
                continue;
            }
            f.drawn = true;
            let o = view.relative(sim.origin);
            let (absorb, scatter) = optics_vectors(&sim.optics);
            let u = FlowUniform {
                origin: [o.x, o.y, o.z, sim.cell],
                dims: [sim.nx as f32, sim.nz as f32, sim.yaw.cos(), sim.yaw.sin()],
                absorb,
                scatter,
                look: [
                    sim.optics.foam,
                    sim.optics.caustics,
                    sim.optics.refraction,
                    0.0,
                ],
            };
            shared
                .queue
                .write_buffer(&f.uniform, 0, bytemuck::bytes_of(&u));
            if f.version != sim.version {
                f.version = sim.version;
                let data = flow_texels(sim);
                write_texture(
                    &shared.queue,
                    &f.texture,
                    f.nx,
                    f.nz,
                    16,
                    bytemuck::cast_slice(&data),
                );
            }
        }
    }

    /// Standing water near the camera from `SurfaceLayers`. Returns the uniform's
    /// (puddle, puddle2) and whether to draw.
    fn upload_puddles(
        &mut self,
        world: &World,
        shared: &GpuShared,
        view: &ViewData,
    ) -> ([f32; 4], [f32; 4], bool) {
        let Some(gpu) = self.gpu.as_ref() else {
            return ([0.0; 4], [0.0; 4], false);
        };
        let Some(layers) = world.get_resource::<SurfaceLayers>() else {
            return ([0.0; 4], [0.0; 4], false);
        };
        let grid = &layers.grid;
        let w = &mut self.puddles;
        let cell = grid.cell;
        let n = PUDDLE_N as usize;
        // Window snapped to the grid's cells around the camera.
        let half = n as f64 * cell as f64 * 0.5;
        let corner =
            ((DVec2::new(view.origin.x, view.origin.z) - DVec2::splat(half) - grid.origin)
                / cell as f64)
                .round();
        let origin = grid.origin + corner * cell as f64;
        let moved = origin != w.origin || cell != w.cell;
        w.origin = origin;
        w.cell = cell;
        w.since_upload += view.delta.max(0.0);
        if moved || w.since_upload >= 0.2 {
            w.since_upload = 0.0;
            let mut data = vec![0f32; n * n * 2];
            let mut wet = false;
            for (idx, px) in data.as_chunks_mut::<2>().0.iter_mut().enumerate() {
                let (i, j) = (
                    corner.x as i64 + (idx % n) as i64,
                    corner.y as i64 + (idx / n) as i64,
                );
                let depth = if i >= 0 && j >= 0 && i < grid.width as i64 && j < grid.height as i64 {
                    grid.data[j as usize * grid.width as usize + i as usize].puddle_m
                } else {
                    0.0
                };
                wet |= depth > 0.0005;
                px[0] = depth;
            }
            w.wet = wet;
            // Dry ground draws no puddles, so it needs no heights (reading them is the costly
            // part: thousands of terrain samples).
            if wet {
                let shifts = world
                    .get_resource::<SharedWater>()
                    .map_or(0, |s| s.0.read().shifts);
                let ground = world.get_resource::<Ground>();
                w.read_ground(ground, (origin, cell, shifts), view.origin.y + 200.0);
                for (px, g) in data.as_chunks_mut::<2>().0.iter_mut().zip(&w.ground) {
                    px[1] = (g - w.base) as f32;
                }
                write_texture(
                    &shared.queue,
                    &gpu.puddle_tex,
                    PUDDLE_N,
                    PUDDLE_N,
                    8,
                    bytemuck::cast_slice(&data),
                );
            }
        }
        let o = view.relative(DVec3::new(w.origin.x, 0.0, w.origin.y));
        (
            [o.x, o.z, cell, PUDDLE_N as f32],
            [(w.base - view.origin.y) as f32, 0.0, 0.0, 0.0],
            w.wet,
        )
    }
}

impl PuddleWindow {
    /// Make `ground` the heights under the window `at`, probing from `probe_y` down. Most
    /// moves are a cell or two as the camera walks: the heights the old window shares with
    /// the new one are kept and only the cells it newly covers are read. A jump reads all of
    /// them, across the cores.
    fn read_ground(&mut self, ground: Option<&Ground>, at: (DVec2, f32, u64), probe_y: f64) {
        if self.ground_at == Some(at) {
            return;
        }
        let n = PUDDLE_N as usize;
        let (origin, cell, _) = at;
        let mut heights = vec![f64::NAN; n * n];
        if let Some((old, old_cell, old_shifts)) = self.ground_at
            && old_cell == cell
            && old_shifts == at.2
            && self.ground.len() == n * n
        {
            let step = ((origin - old) / cell as f64).round();
            if step.x.abs() < n as f64 && step.y.abs() < n as f64 {
                let (di, dj) = (step.x as i64, step.y as i64);
                for (idx, h) in heights.iter_mut().enumerate() {
                    let (i, j) = ((idx % n) as i64 + di, (idx / n) as i64 + dj);
                    if i >= 0 && j >= 0 && i < n as i64 && j < n as i64 {
                        *h = self.ground[j as usize * n + i as usize];
                    }
                }
            }
        }
        let sample = |idx: usize, h: &mut f64| {
            if h.is_nan() {
                let p = origin
                    + DVec2::new((idx % n) as f64 + 0.5, (idx / n) as f64 + 0.5) * cell as f64;
                *h = ground
                    .and_then(|g| g.height(DVec3::new(p.x, probe_y, p.y)))
                    .unwrap_or(0.0);
            }
        };
        if heights.iter().filter(|h| h.is_nan()).count() > 4 * n {
            let rows = 8;
            engine_core::tasks::init_task_pools();
            engine_core::tasks::par_chunks_mut(&mut heights, n * rows, |k, chunk| {
                for (o, h) in chunk.iter_mut().enumerate() {
                    sample(k * n * rows + o, h);
                }
            });
        } else {
            for (idx, h) in heights.iter_mut().enumerate() {
                sample(idx, h);
            }
        }
        self.base = heights.iter().copied().fold(f64::INFINITY, f64::min);
        if !self.base.is_finite() {
            self.base = 0.0;
        }
        self.ground = heights;
        self.ground_at = Some(at);
    }
}

impl RenderFeature for WaterFeature {
    fn name(&self) -> &str {
        "water"
    }

    fn order(&self) -> i32 {
        -10
    }

    fn init(&mut self, gpu: &GpuShared) {
        self.gpu = Some(Self::build(gpu));
    }

    fn resize(&mut self, _gpu: &GpuShared, _width: u32, _height: u32) {
        if let Some(g) = &mut self.gpu {
            g.group1 = None;
        }
    }

    fn prepare(
        &mut self,
        world: &mut World,
        shared: &GpuShared,
        view: &ViewData,
        _settings: &RenderSettings,
    ) {
        self.bodies.clear();
        self.underwater = false;
        self.stats = FeatureStats::default();
        if self.gpu.is_none() {
            return;
        }
        let Some(water) = world.get_resource::<SharedWater>().cloned() else {
            return;
        };
        // Local rain (showers).
        let showers_q = self.showers.get_or_insert_with(|| world.query());
        let cam = view.origin;
        let mut showers: Vec<(f64, [f32; 4])> = showers_q
            .iter(world)
            .map(|(g, s)| {
                let p = g.translation();
                let d = DVec2::new(p.x - cam.x, p.z - cam.z).length() - s.radius_m as f64;
                (
                    d,
                    [
                        (p.x - cam.x) as f32,
                        (p.z - cam.z) as f32,
                        s.radius_m,
                        s.mm_h,
                    ],
                )
            })
            .collect();
        showers.sort_by(|a, b| a.0.total_cmp(&b.0));
        let rain = world
            .get_resource::<WeatherState>()
            .filter(|w| w.is_raining())
            .map_or(0.0, |w| w.precipitation_mm_h);
        let span = tracing::info_span!("water", name = "puddles").entered();
        let (puddle, puddle2, puddles_on) = self.upload_puddles(world, shared, view);
        drop(span);
        let surface_damping = world
            .get_resource::<crate::body::WaterSettings>()
            .map_or(0.05, |s| s.surface_damping);

        let state = water.0.read();
        let gpu = self.gpu.as_mut().expect("checked above");
        let mut uniform = WaterUniform::zeroed();

        // Nearest bodies first.
        let cam_xz = DVec2::new(cam.x, cam.z);
        let mut order: Vec<(f64, usize)> = state
            .bodies
            .iter()
            .enumerate()
            .filter(|(_, b)| b.body.visible)
            .map(|(i, b)| (extent_range(b, cam_xz).0, i))
            .collect();
        order.sort_by(|a, b| a.0.total_cmp(&b.0));
        // The fine waves follow the nearest sea with wind waves.
        let sea = order
            .iter()
            .map(|&(_, i)| detail_sea(&state.bodies[i]))
            .find(|s| s.is_some_and(|s| crate::detail::Spectrum::of(&s).is_some()))
            .flatten();
        // Without one, streams still take the ripples of the breeze.
        let breeze = || {
            let wind = engine_core::math::Vec2::new(view.wind.x, view.wind.z);
            crate::detail::DetailSea {
                wind: wind.length().max(3.0),
                dir: wind.try_normalize().unwrap_or(engine_core::math::Vec2::X),
                fetch_m: 50.0,
                height_scale: 1.0,
                choppiness: 0.5,
            }
        };
        gpu.detail.update(
            shared,
            Some(sea.unwrap_or_else(breeze)),
            view.time,
            view.delta,
        );
        let tiles = gpu.detail.state.filter(|_| gpu.detail.active);
        let mut under: Option<(usize, f32)> = None;
        for &(_, i) in order.iter() {
            if self.bodies.len() >= MAX_BODIES {
                break;
            }
            let b = &state.bodies[i];
            let Some((mut body, draw)) = Self::body_uniform(b, view) else {
                continue;
            };
            body.detail = body_detail(b, tiles.as_ref());
            let slot = self.bodies.len();
            if under.is_none()
                && let Some((height, depth, ..)) = state.sample_body(b, cam)
                && cam.y < height - 0.02
                && cam.y > height - depth as f64 - 0.5
            {
                under = Some((slot, (height - cam.y) as f32));
            }
            uniform.bodies[slot] = body;
            self.bodies.push(draw);
        }

        let r = &state.ripples;
        let ripple_rel = view.relative(DVec3::new(r.origin.x, 0.0, r.origin.y));
        let mut g = Globals::zeroed();
        g.ripple = [
            ripple_rel.x,
            ripple_rel.z,
            r.size() as f32,
            if r.has_water { 1.0 } else { 0.0 },
        ];
        g.rain = [rain, view.time, -1.0, 0.0];
        for (dst, (_, s)) in g.showers.iter_mut().zip(&showers) {
            *dst = *s;
        }
        g.puddle = puddle;
        g.puddle2 = puddle2;
        g.counts = [
            self.bodies.len() as u32,
            showers.len().min(MAX_SHOWERS) as u32,
            puddles_on as u32,
            0,
        ];
        // Where the falls land, the water boils white (`plunge_boil`).
        let plunges: Vec<(DVec3, Vec2, f32)> = state
            .flows
            .iter()
            .filter(|sim| sim.visible)
            .flat_map(|sim| sim.falls().plunges.iter().copied())
            .collect();
        let mut near: Vec<_> = plunges
            .iter()
            .map(|(foot, _, power)| (foot.distance(view.origin), *foot, *power))
            .filter(|(d, ..)| *d < PLUNGE_REACH)
            .collect();
        near.sort_by(|a, b| a.0.total_cmp(&b.0));
        for (dst, (_, foot, power)) in g.plunges.iter_mut().zip(&near) {
            *dst = view.relative(*foot).extend(boil_radius(*power)).to_array();
        }
        g.counts[3] = near.len().min(MAX_PLUNGES) as u32;
        let frame_wind = engine_core::math::Vec2::new(view.wind.x, view.wind.z);
        (g.detail, g.detail2) = gpu.detail.globals(frame_wind);
        let span = tracing::info_span!("water", name = "surface").entered();
        gpu.surface.update(
            shared,
            &state.ripples,
            state.shifts,
            view.delta,
            surface_damping,
        );
        // The ripples' heights come from the dispersive surface while it runs.
        g.detail[3] = if gpu.surface.drawn() { 1.0 } else { 0.0 };
        if let Some((corner, size)) = gpu.surface.coarse_drawn() {
            let rel = view.relative(DVec3::new(corner.x, 0.0, corner.y));
            g.coarse = [rel.x, rel.z, size, 1.0];
        }
        if let Some((slot, depth)) = under {
            let b = &uniform.bodies[slot];
            g.rain[2] = slot as f32;
            g.rain[3] = depth;
            g.under = b.absorb;
            g.under2 = [b.scatter[0], b.scatter[1], b.scatter[2], depth];
            self.underwater = true;
        }
        uniform.g = g;
        shared
            .queue
            .write_buffer(&gpu.uniform, 0, bytemuck::bytes_of(&uniform));

        drop(span);
        let span = tracing::info_span!("water", name = "ripple upload").entered();
        let heights = !gpu.surface.drawn();
        Self::upload_ripples(gpu, shared, &state, heights);
        drop(span);
        let span = tracing::info_span!("water", name = "flow upload").entered();
        Self::upload_flows(gpu, shared, &state, view);
        drop(span);
        Self::upload_falls(gpu, shared, &state, view, &plunges);

        // Stats.
        let flows = gpu.flows.iter().filter(|f| f.drawn);
        self.stats.draw_calls = self.bodies.len() as u32
            + flows.clone().count() as u32
            + puddles_on as u32
            + self.underwater as u32
            + (gpu.fall_count > 0) as u32;
        self.stats.instances = self.bodies.len() as u32;
        self.stats.triangles = self
            .bodies
            .iter()
            .map(|d| ((d.last_band - d.first_band) * SEGMENTS * 2) as u64)
            .sum::<u64>()
            + flows.map(|f| f.index_count as u64 / 3).sum::<u64>()
            + gpu.fall_count as u64 / 3
            + if puddles_on {
                ((PUDDLE_N - 1) * (PUDDLE_N - 1) * 2) as u64
            } else {
                0
            };
        self.puddles.wet = puddles_on;
    }

    fn compute(&mut self, encoder: &mut wgpu::CommandEncoder, ctx: &FrameContext) {
        let Some(gpu) = &mut self.gpu else {
            return;
        };
        gpu.detail.dispatch(encoder);
        gpu.surface.dispatch(encoder);
        // Built again only when what it binds is made anew: the scene copies on resize, the
        // ripple textures when the grid changes size (both clear it).
        if gpu.group1.is_some() {
            return;
        }
        let ripple = gpu.ripple.create_view(&Default::default());
        let currents = gpu.currents.create_view(&Default::default());
        let puddle = gpu.puddle_tex.create_view(&Default::default());
        gpu.group1 = Some(
            ctx.gpu
                .device
                .create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("water"),
                    layout: &gpu.group1_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: gpu.uniform.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::TextureView(&ripple),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: wgpu::BindingResource::TextureView(ctx.scene_color_copy),
                        },
                        wgpu::BindGroupEntry {
                            binding: 3,
                            resource: wgpu::BindingResource::TextureView(ctx.scene_depth_copy),
                        },
                        wgpu::BindGroupEntry {
                            binding: 4,
                            resource: wgpu::BindingResource::TextureView(&puddle),
                        },
                        wgpu::BindGroupEntry {
                            binding: 5,
                            resource: wgpu::BindingResource::TextureView(&currents),
                        },
                        wgpu::BindGroupEntry {
                            binding: 6,
                            resource: wgpu::BindingResource::TextureView(&gpu.detail.view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 7,
                            resource: wgpu::BindingResource::TextureView(gpu.surface.view()),
                        },
                        wgpu::BindGroupEntry {
                            binding: 8,
                            resource: wgpu::BindingResource::TextureView(gpu.surface.coarse_view()),
                        },
                    ],
                }),
        );
    }

    fn draw(&self, phase: RenderPhase, pass: &mut wgpu::RenderPass<'_>, _ctx: &FrameContext) {
        if phase != RenderPhase::Transparent {
            return;
        }
        let Some(gpu) = &self.gpu else {
            return;
        };
        let Some(group1) = &gpu.group1 else {
            return;
        };
        pass.set_bind_group(1, group1, &[]);
        if self.underwater {
            pass.set_pipeline(&gpu.under);
            pass.draw(0..3, 0..1);
        }
        if self.puddles.wet {
            pass.set_pipeline(&gpu.puddle);
            pass.set_index_buffer(gpu.puddle_index.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..(PUDDLE_N - 1) * (PUDDLE_N - 1) * 6, 0, 0..1);
        }
        if gpu.flows.iter().any(|f| f.drawn) {
            pass.set_pipeline(&gpu.flow);
            for f in gpu.flows.iter().filter(|f| f.drawn) {
                pass.set_bind_group(2, &f.group, &[]);
                pass.set_index_buffer(f.index.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..f.index_count, 0, 0..1);
            }
        }
        if !self.bodies.is_empty() {
            pass.set_pipeline(&gpu.body);
            pass.set_index_buffer(gpu.grid_index.slice(..), wgpu::IndexFormat::Uint32);
            for (i, d) in self.bodies.iter().enumerate() {
                let per_band = SEGMENTS * 6;
                pass.draw_indexed(
                    d.first_band * per_band..d.last_band * per_band,
                    0,
                    i as u32..i as u32 + 1,
                );
            }
        }
        if gpu.fall_count > 0 {
            pass.set_pipeline(&gpu.falls);
            pass.set_vertex_buffer(0, gpu.fall_buffer.slice(..));
            pass.draw(0..gpu.fall_count, 0..1);
        }
    }

    fn needs_scene_copy(&self) -> bool {
        !self.bodies.is_empty()
            || self.puddles.wet
            || self
                .gpu
                .as_ref()
                .is_some_and(|g| g.fall_count > 0 || g.flows.iter().any(|f| f.drawn))
    }

    fn stats(&self) -> FeatureStats {
        self.stats
    }

    fn describe(&self) -> serde_json::Value {
        serde_json::json!({
            "bodies": self.bodies.len(),
            "bands": self.bodies.iter().map(|d| d.last_band - d.first_band).collect::<Vec<_>>(),
            "underwater": self.underwater,
            "puddles": self.puddles.wet,
            "flows": self.gpu.as_ref().map_or(0, |g| g.flows.iter().filter(|f| f.drawn).count()),
            "fall_triangles": self.gpu.as_ref().map_or(0, |g| g.fall_count / 3),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shader_validates() {
        let code = engine_render::features::compose_shader(SHADER, true);
        engine_render::features::validate_wgsl("water", &code).unwrap_or_else(|e| panic!("{e}"));
    }

    #[test]
    fn uniform_layout_matches_the_shader() {
        // WGSL: Globals 10 vec4 + 4 showers + 8 plunges; GpuBody 11 vec4 + 24 waves of 2 vec4.
        assert_eq!(std::mem::size_of::<Globals>(), (10 + 4 + 8) * 16);
        assert_eq!(std::mem::size_of::<GpuBody>(), (11 + MAX_WAVES * 2) * 16);
        assert_eq!(
            std::mem::size_of::<WaterUniform>(),
            std::mem::size_of::<Globals>() + MAX_BODIES * std::mem::size_of::<GpuBody>()
        );
        assert_eq!(std::mem::size_of::<FlowUniform>(), 5 * 16);
    }

    #[test]
    fn rings_cover_the_requested_radius() {
        let (s0, alpha) = (0.05, TAU / SEGMENTS as f64);
        for r in [0.5, 10.0, 300.0, 20_000.0] {
            let i = ring_at(r, s0, alpha);
            assert!((ring_radius(i, s0, alpha) - r).abs() < 1e-6 * r.max(1.0));
        }
        assert!(ring_radius(MAX_RINGS as f64, s0, alpha) > OCEAN_REACH);
    }

    #[test]
    fn a_curtain_falls_from_its_lip_to_its_foot() {
        let lip = crate::flow::Lip {
            top: DVec3::new(10.0, 9.0, 0.0),
            dir: Vec2::X,
            speed: 2.0,
            q: 0.2,
            drop: 8.0,
            width: 0.75,
        };
        // Its free-fall arc down 8.4 m (as `FlowSim::fall_path` gives it, evenly in time here).
        let t_end = (2.0 * 8.4f32 / 9.81).sqrt();
        let path: Vec<crate::flow::FallPoint> = (0..=FALL_SEGMENTS)
            .map(|k| {
                let t = t_end * k as f32 / FALL_SEGMENTS as f32;
                crate::flow::FallPoint {
                    pos: lip.top + DVec3::new(2.0 * t as f64, -0.5 * 9.81 * (t * t) as f64, 0.0),
                    vel: Vec3::new(2.0, -9.81 * t, 0.0),
                    t,
                    rock: Vec3::ZERO,
                }
            })
            .collect();
        let (mut v, mut wet) = (Vec::new(), Vec::new());
        let camera = DVec3::new(9.0, 7.0, -3.0);
        fall_strip(
            &lip,
            &path,
            [false, true],
            |p| (p - camera).as_vec3(),
            &mut v,
            &mut wet,
        );
        // Falling free it wets no rock.
        assert!(wet.is_empty());
        // The core (from a little before the brink), and the spray in front of and behind it.
        assert_eq!(v.len(), (FALL_SEGMENTS + 1) * 6 + FALL_SEGMENTS * 6 * 2);
        let (core, shells): (Vec<FallVertex>, Vec<FallVertex>) =
            v.iter().partition(|f| f.uv_speed[3] == 0.0);
        assert_eq!(core.len(), (FALL_SEGMENTS + 1) * 6);
        assert!(shells.iter().all(|f| f.uv_speed[3] == SHELL_KIND));
        // Only the end no other lip continues frays.
        assert!(
            v.iter()
                .all(|f| f.edge[0] == if f.across_side[3] < 0.0 { 1.0 } else { 0.0 })
        );
        let pos = |f: &FallVertex| Vec3::from_slice(&f.rel_q[..3]);
        // The spray spreads wider than the core as the jet falls (`jet_spread`), and stands
        // off it to both sides.
        let width = |set: &[FallVertex]| {
            set.iter()
                .filter(|f| f.normal_frac[3] == 1.0)
                .map(|f| pos(f).z)
                .fold((f32::MAX, f32::MIN), |(lo, hi), z| (lo.min(z), hi.max(z)))
        };
        let (c, s) = (width(&core), width(&shells));
        assert!(s.1 - s.0 > c.1 - c.0 + 0.2, "core {c:?}, spray {s:?}");
        let v = core;
        // Starts just short of the lip, the water level with it rolling over it, a strip as
        // wide as the lip (a little more, to overlap).
        let first = &v[0];
        assert!(first.normal_frac[3] == 0.0 && first.normal_frac[1] > 0.99);
        assert!(first.uv_speed[1] < 0.0);
        let start = Vec3::new(1.0 - APPROACH_M, 2.015, 3.0 - 0.75 * 0.66);
        assert!((pos(first) - start).length() < 1e-4, "{}", pos(first));
        let brink = v.iter().find(|f| f.uv_speed[1] == 0.0).unwrap();
        assert!((pos(brink) - Vec3::new(1.0, 2.0, 3.0 - 0.75 * 0.66)).length() < 1e-4);
        // Ends where it lands, having carried on outward on its arc.
        let last = v
            .iter()
            .max_by(|a, b| a.normal_frac[3].total_cmp(&b.normal_frac[3]));
        let end = pos(last.unwrap());
        assert!((end.y - (2.0 - 8.4)).abs() < 1e-3, "{end}");
        let t = (2.0 * 8.4f32 / 9.81).sqrt();
        assert!((end.x - (1.0 + 2.0 * t)).abs() < 1e-3, "{end}");
        // Every triangle faces outward or up, never into the cliff, and is wound so its front
        // face is that side.
        for f in &v {
            let n = Vec3::from_slice(&f.normal_frac[..3]);
            assert!(n.x >= -1e-4 && (n.length() - 1.0).abs() < 1e-3, "{n}");
        }
        for tri in v.chunks(3) {
            let [a, b, c] = [pos(&tri[0]), pos(&tri[1]), pos(&tri[2])];
            let n = Vec3::from_slice(&tri[0].normal_frac[..3]);
            assert!((b - a).cross(c - a).dot(n) > 0.0);
        }
    }

    #[test]
    fn a_sheet_sliding_down_rock_wets_a_wider_band_of_it() {
        let lip = crate::flow::Lip {
            top: DVec3::new(0.0, 5.0, 0.0),
            dir: Vec2::X,
            speed: 1.0,
            q: 0.05,
            drop: 4.0,
            width: 0.5,
        };
        // Down a 60° slab facing +x.
        let rock = Vec3::new(0.866, 0.5, 0.0);
        let path: Vec<crate::flow::FallPoint> = (0..=FALL_SEGMENTS)
            .map(|k| {
                let s = 8.0 * k as f32 / FALL_SEGMENTS as f32;
                crate::flow::FallPoint {
                    pos: lip.top + DVec3::new(0.5 * s as f64, -0.866 * s as f64 * 0.577, 0.0),
                    vel: Vec3::new(1.0, -1.7, 0.0),
                    t: 0.2 * k as f32,
                    rock: if k == 0 { Vec3::ZERO } else { rock },
                }
            })
            .collect();
        let (mut v, mut wet) = (Vec::new(), Vec::new());
        fall_strip(&lip, &path, [true, true], |p| p.as_vec3(), &mut v, &mut wet);
        assert_eq!(wet.len(), (FALL_SEGMENTS - 1) * 6);
        assert!(
            wet.iter()
                .all(|f| f.uv_speed[3] == WET_KIND && f.edge[0] == 0.0)
        );
        let half = |set: &[FallVertex], at: f32| {
            set.iter()
                .filter(|f| f.normal_frac[3] == at)
                .map(|f| f.rel_q[2].abs())
                .fold(0.0f32, f32::max)
        };
        let sheet: Vec<_> = v
            .iter()
            .copied()
            .filter(|f| f.uv_speed[3] == ROCK_KIND)
            .collect();
        assert!(half(&wet, 1.0) > half(&sheet, 1.0) + 0.2);
        // Under the sheet, on the rock side of it.
        let (w, c) = (wet.last().unwrap(), sheet.last().unwrap());
        let d = Vec3::from_slice(&c.rel_q[..3]) - Vec3::from_slice(&w.rel_q[..3]);
        assert!(d.dot(rock) > 0.005, "{d}");
    }

    #[test]
    fn spray_clouds_face_the_camera_over_the_foot() {
        let (right, up) = (Vec3::X, Vec3::Y);
        let mut v = Vec::new();
        let foot = Vec3::new(0.0, -5.0, -20.0);
        // A small stream falling 8 m (0.2 m³/s): about 16 kW.
        mist_puff(
            DVec3::new(3.0, 1.0, 4.0),
            foot,
            16_000.0,
            Vec2::ZERO,
            right,
            up,
            &mut v,
        );
        // The cloud on the pool and two layers of plume over it, each rising and spreading.
        assert_eq!(v.len(), 18);
        let pos = |f: &FallVertex| Vec3::from_slice(&f.rel_q[..3]);
        let mut below = (foot.y, 0.0);
        for layer in v.chunks(6) {
            let center = layer.iter().map(pos).sum::<Vec3>() / 6.0;
            let radius = layer[0].normal_frac[2];
            assert!(center.y > below.0 && radius > below.1, "{center} {radius}");
            assert!((center.with_y(0.0) - foot.with_y(0.0)).length() < 1e-4);
            below = (center.y, radius);
        }
        let radius = v[0].normal_frac[2];
        assert!((1.5..3.0).contains(&radius), "{radius}");
        for f in &v {
            assert_eq!(f.uv_speed[3], 1.0);
            assert!((pos(f).z - foot.z).abs() < 1e-4, "not facing the camera");
        }
    }

    #[test]
    fn grids_face_up() {
        // Positions of a 3×3 vertex grid (x = i, z = j); every triangle's normal points +y.
        // Either diagonal: a valley along +x+z on the second grid flips every quad.
        let valley: Vec<f32> = (0..9)
            .map(|v| ((v % 3) as f32 - (v / 3) as f32).abs())
            .collect();
        for idx in [grid_indices(3, 3, None), grid_indices(3, 3, Some(&valley))] {
            let pos = |v: u32| ((v % 3) as f32, (v / 3) as f32);
            for t in idx.chunks(3) {
                let (a, b, c) = (pos(t[0]), pos(t[1]), pos(t[2]));
                let (u, v) = ((b.0 - a.0, b.1 - a.1), (c.0 - a.0, c.1 - a.1));
                // y of (u × v) with u, v in (x, z): u.z * v.x − u.x * v.z.
                assert!(u.1 * v.0 - u.0 * v.1 > 0.0);
            }
        }
        assert_ne!(grid_indices(3, 3, None), grid_indices(3, 3, Some(&valley)));
    }
}
