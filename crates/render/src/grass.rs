//! Grass: meadows of individually drawn blades that sway in the wind, with gusts rolling across
//! them, part around characters ([`GrassBender`]) and thin out smoothly with distance.
//!
//! A [`GrassField`] covers a rectangle of ground on its entity's local x/z plane (the entity's
//! rotation tilts it, for example onto a planet's surface) and carries a map of ground height,
//! density and dryness on a grid. Nothing per blade is kept between frames: a compute pass
//! (`shaders/grass_cull.wgsl`) grows every blade of the tiles around the camera from its
//! tile's grid coordinates and its index, once a frame, so the same blade grows in the same
//! spot every frame and a field of millions of blades costs a small texture. It drops blades
//! thinned out by distance or outside the view and writes one 64-byte record per kept blade;
//! the depth prepass (so ambient occlusion, bounce light and anti-aliasing see them), the
//! scene and, near the camera, the sun's shadow cascades draw the records with indirect draws,
//! their vertex shaders only bending each blade along its curve (`shaders/grass_common.wgsl`).
//!
//! ```text
//! prepare: fields → map textures (on change); tiles around the camera → cull (view frustum,
//!          cascades) → blade count from the nearest point's LOD → tiles, cull jobs (64 blades
//!          each) and the draw groups' sizes
//! compute: one thread a blade → grown, culled, its tip in the wind → record in its group
//! draw:    one indirect triangle-strip draw per group: the screen's by segments (1-6), each
//!          cascade's by segments (1-2)
//! ```

use std::collections::HashMap;

use bevy_ecs::prelude::*;
use bytemuck::{Pod, Zeroable};
use engine_core::prelude::*;
use serde::{Deserialize, Serialize};

use crate::features::{
    FeatureStats, FrameContext, GpuShared, GrowBuffer, PrepareOut, RenderFeature, RenderPhase,
};
use crate::frame::ViewData;
use crate::renderer::layout_entry;
use crate::settings::RenderSettings;
use crate::shadows::MAX_CASCADES;

/// Tile edge, meters (must match `GRASS_TILE` in the shader).
const TILE: f32 = 8.0;
/// Instance index bits for the blade within its tile.
const BLADE_BITS: u32 = 16;
const MAX_BLADES_PER_TILE: u32 = (1 << BLADE_BITS) - 1;
const MAX_BENDERS: usize = 16;
const MAX_BLADES_PER_M2: f32 = 1000.0;
/// Blades a cull job covers (`@workgroup_size` in grass_cull.wgsl).
const CULL_CHUNK: u32 = 64;
/// Bytes of one blade's record (`GrassRecord` in grass_common.wgsl).
const RECORD_BYTES: u64 = 64;
/// Indirect draws of the blades kept for the screen: one for each count of segments, 1..=6.
const SCREEN_GROUPS: usize = 6;
/// Segments a shadow caster can have (`shadow_segments_at`).
const SHADOW_SEGMENTS: usize = 2;
/// Indirect draws: the screen's, then each cascade's by segments.
const GROUPS: usize = SCREEN_GROUPS + MAX_CASCADES * SHADOW_SEGMENTS;
/// A dispatch's workgroups along x at most (the rest go along y).
const MAX_GROUPS_X: u32 = 65535;

/// A meadow: grass blades over a rectangle of ground.
///
/// The rectangle is `size` meters along the entity's local x and z, centered on the entity.
/// `heights`, `density` and `dryness` are maps on a `resolution[0] × resolution[1]` grid over
/// it (row by row along x, the first point at the -x -z corner, the last at the +x +z corner);
/// an empty map means flat ground, grass everywhere and `style.dryness`.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[require(Transform)]
#[serde(default)]
pub struct GrassField {
    /// Size along the entity's local x and z, meters.
    pub size: [f32; 2],
    /// Grid points along x and z of the maps.
    pub resolution: [u32; 2],
    /// Ground height at each grid point, meters along the entity's local up.
    pub heights: Vec<f32>,
    /// How thickly grass grows at each grid point, 0..1 (short and sparse toward 0).
    pub density: Vec<f32>,
    /// 0 = fresh green .. 1 = dry straw at each grid point.
    pub dryness: Vec<f32>,
    pub style: GrassStyle,
    /// Which grass grows at each grid point: 0 `style`, `k` `styles[k - 1]` (empty: `style`
    /// everywhere). A blade takes the kind of the point nearest a spot about a meter around
    /// it, so kinds meet in ragged drifts rather than along the grid; all kinds grow in the
    /// same pass. A meadow of turf, tall grass, sedge and flowers is one field (Ghost of
    /// Tsushima's grass-type map).
    pub kinds: Vec<u8>,
    /// The other kinds' looks (at most [`MAX_GRASS_KINDS`] - 1). Each keeps its own
    /// `blades_per_m2`; the field's draw and shadow distances are `style`'s.
    pub styles: Vec<GrassStyle>,
}

/// Kinds of grass one [`GrassField`] can grow: its `style` and up to 15 `styles`.
pub const MAX_GRASS_KINDS: usize = 16;

impl Default for GrassField {
    fn default() -> Self {
        Self {
            size: [40.0, 40.0],
            resolution: [0, 0],
            heights: Vec::new(),
            density: Vec::new(),
            dryness: Vec::new(),
            style: GrassStyle::default(),
            kinds: Vec::new(),
            styles: Vec::new(),
        }
    }
}

impl GrassField {
    /// A flat meadow `x` by `z` meters.
    pub fn flat(x: f32, z: f32) -> Self {
        Self {
            size: [x, z],
            ..Default::default()
        }
    }

    /// Fill the maps by sampling `f(x, z) -> (height, density, dryness)` at every grid point
    /// (`x`, `z` in meters from the field's center along its local axes).
    pub fn sample(
        mut self,
        resolution: [u32; 2],
        mut f: impl FnMut(f32, f32) -> (f32, f32, f32),
    ) -> Self {
        let [rx, rz] = resolution.map(|r| r.max(2));
        self.resolution = [rx, rz];
        let n = (rx * rz) as usize;
        self.heights = Vec::with_capacity(n);
        self.density = Vec::with_capacity(n);
        self.dryness = Vec::with_capacity(n);
        for j in 0..rz {
            for i in 0..rx {
                let x = (i as f32 / (rx - 1) as f32 - 0.5) * self.size[0];
                let z = (j as f32 / (rz - 1) as f32 - 0.5) * self.size[1];
                let (h, d, dry) = f(x, z);
                self.heights.push(h);
                self.density.push(d);
                self.dryness.push(dry);
            }
        }
        self
    }

    pub fn with_style(mut self, style: GrassStyle) -> Self {
        self.style = style;
        self
    }

    /// Every kind's look: `style`, then `styles` (at most [`MAX_GRASS_KINDS`]).
    pub fn palette(&self) -> impl Iterator<Item = &GrassStyle> {
        std::iter::once(&self.style).chain(self.styles.iter().take(MAX_GRASS_KINDS - 1))
    }
}

/// Where a [`GrassField`]'s ground is worn, on its entity: blades there shorten, lie flat and
/// dry toward straw, thin out, and are gone where it is worn bare, so a path reads as trodden
/// earth. A map finer than the field's own over a rectangle of it, so it can follow a footpath
/// that the field's map would blur. (`engine_foliage` keeps one up to date on every field from
/// `engine_world::GroundWear`.)
#[derive(Component, Debug, Clone, PartialEq, Default)]
pub struct GrassWear {
    /// The map's first point, meters from the field's -x -z corner along its x and z.
    pub origin: [f32; 2],
    /// Meters between map points.
    pub cell: f32,
    /// Map points along x and z.
    pub resolution: [u32; 2],
    /// 0 untouched .. 1 worn bare at each point, row by row along x.
    pub wear: Vec<f32>,
}

/// How the blades of a [`GrassField`] look and behave.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GrassStyle {
    /// Blades per square meter where density is 1 (at most 1000).
    pub blades_per_m2: f32,
    /// Typical blade height, meters.
    pub height: f32,
    /// 0..1: random spread of blade heights.
    pub height_variation: f32,
    /// Blade width at the root, meters.
    pub width: f32,
    /// How far blades lean over: 0 = straight up, 1 = about 45 degrees.
    pub bend: f32,
    /// Albedo (linear RGB) of fresh blades.
    pub lush_color: [f32; 3],
    /// Albedo (linear RGB) of dry blades.
    pub dry_color: [f32; 3],
    /// Dryness where the field has no dryness map.
    pub dryness: f32,
    /// How much the wind moves the blades (1 = normal, 0 = still).
    pub wind_response: f32,
    /// Blades thin out with distance and are gone at this distance, meters.
    pub draw_distance: f32,
    /// Blades cast sun shadows up to this distance from the camera, meters (0 = never).
    pub shadow_distance: f32,
    /// Blades grow in clumps about this far apart, meters (0 = evenly, no clumps).
    pub clump_size: f32,
    /// 0..1: how far blades crowd in toward their clump's center (tufts and tussocks).
    pub clump_pull: f32,
    /// 0..1: how much blades turn to fan out from their clump's center.
    pub clump_fan: f32,
    /// 0..1: how much clumps differ in height.
    pub clump_height: f32,
    /// 0..1: how much clumps differ in color (keep it small: a few percent reads natural).
    pub clump_color: f32,
    /// 0..1: how far leaning tips droop under their own weight. Scale it with gravity: about
    /// 0.3 on Earth, less on a light world, more on a heavy one.
    pub droop: f32,
    /// 0..1: sheen along the blades' midribs up close (0 = matte).
    pub gloss: f32,
    /// Tint (× the blade's color) toward the tips: paler and yellower on most grasses.
    pub tip_tint: [f32; 3],
    /// 0..1: grow leaves instead of blades: each a leaf on a stalk, ovate past it (`width` is
    /// then the leaf's widest, `height` its length with the stalk). A tight clump of them is a
    /// rosette ([`GrassStyle::forest_herbs`]).
    pub leaf: f32,
    /// Fraction of a leaf's length that is stalk (with `leaf`).
    pub stalk: f32,
    /// 0..1: leaves lie flat on the ground in their own direction, some curling up at the tip:
    /// fallen leaves ([`GrassStyle::leaf_litter`]).
    pub lying: f32,
    /// 0..1: share of the blades that are flowering stems: a little taller and straighter than
    /// the blades, a head of `bloom_color` at the top (buttercups, daisies, clover heads).
    pub bloom: f32,
    /// Albedo (linear RGB) of the flower heads.
    pub bloom_color: [f32; 3],
    /// Width of a flower head, meters.
    pub bloom_size: f32,
}

impl Default for GrassStyle {
    fn default() -> Self {
        Self {
            blades_per_m2: 400.0,
            height: 0.4,
            height_variation: 0.4,
            width: 0.012,
            bend: 0.35,
            lush_color: [0.075, 0.16, 0.035],
            dry_color: [0.36, 0.30, 0.14],
            dryness: 0.15,
            wind_response: 1.0,
            draw_distance: 70.0,
            shadow_distance: 20.0,
            clump_size: 0.7,
            clump_pull: 0.2,
            clump_fan: 0.5,
            clump_height: 0.35,
            clump_color: 0.06,
            droop: 0.3,
            gloss: 0.35,
            tip_tint: [1.35, 1.2, 0.7],
            leaf: 0.0,
            stalk: 0.0,
            lying: 0.0,
            bloom: 0.0,
            bloom_color: [0.8, 0.7, 0.1],
            bloom_size: 0.025,
        }
    }
}

impl GrassStyle {
    /// Low shade herbs of a forest floor (wood sorrel, clover, wild strawberry): rosettes of
    /// leaves on stalks a hand high, swaying a little.
    pub fn forest_herbs() -> Self {
        Self {
            blades_per_m2: 240.0,
            height: 0.22,
            height_variation: 0.35,
            width: 0.075,
            bend: 2.0,
            lush_color: [0.075, 0.17, 0.03],
            dry_color: [0.2, 0.16, 0.05],
            dryness: 0.1,
            wind_response: 0.35,
            draw_distance: 28.0,
            shadow_distance: 8.0,
            clump_size: 0.36,
            clump_pull: 0.95,
            clump_fan: 1.0,
            clump_height: 0.4,
            clump_color: 0.1,
            droop: 0.6,
            gloss: 0.45,
            tip_tint: [1.0, 1.0, 1.0],
            leaf: 1.0,
            stalk: 0.4,
            lying: 0.0,
            ..Default::default()
        }
    }

    /// Fallen leaves lying on the ground: browns, with paler ochre ones where `dryness` is high.
    pub fn leaf_litter() -> Self {
        Self {
            blades_per_m2: 320.0,
            height: 0.09,
            height_variation: 0.4,
            width: 0.055,
            bend: 0.0,
            lush_color: [0.07, 0.04, 0.018],
            dry_color: [0.17, 0.1, 0.04],
            dryness: 0.4,
            wind_response: 0.0,
            draw_distance: 16.0,
            shadow_distance: 0.0,
            clump_size: 0.5,
            clump_pull: 0.25,
            clump_fan: 0.0,
            clump_height: 0.2,
            clump_color: 0.25,
            droop: 0.0,
            gloss: 0.12,
            tip_tint: [0.92, 0.85, 0.8],
            leaf: 1.0,
            stalk: 0.08,
            lying: 1.0,
            ..Default::default()
        }
    }

    /// A temperate summer meadow: knee-high, fresh green, yellowing at the tips.
    pub fn meadow() -> Self {
        Self {
            blades_per_m2: 520.0,
            height: 0.38,
            height_variation: 0.5,
            width: 0.011,
            bend: 0.38,
            lush_color: [0.085, 0.17, 0.04],
            dry_color: [0.26, 0.22, 0.09],
            dryness: 0.15,
            clump_size: 0.6,
            clump_pull: 0.18,
            clump_fan: 0.5,
            clump_height: 0.35,
            clump_color: 0.05,
            droop: 0.3,
            gloss: 0.35,
            tip_tint: [1.3, 1.2, 0.75],
            ..Default::default()
        }
    }

    /// Savanna: tall, sparse, sun-bleached tussocks.
    pub fn savanna() -> Self {
        Self {
            blades_per_m2: 260.0,
            height: 0.85,
            height_variation: 0.45,
            width: 0.009,
            bend: 0.55,
            lush_color: [0.2, 0.19, 0.07],
            dry_color: [0.46, 0.36, 0.17],
            dryness: 0.7,
            // Tussocks: blades crowded into fans, tips bleached.
            clump_size: 0.9,
            clump_pull: 0.6,
            clump_fan: 0.85,
            clump_height: 0.5,
            clump_color: 0.08,
            droop: 0.45,
            gloss: 0.2,
            tip_tint: [1.25, 1.15, 0.9],
            ..Default::default()
        }
    }

    /// Tundra: short, dense, olive and russet sedges.
    pub fn tundra() -> Self {
        Self {
            blades_per_m2: 800.0,
            height: 0.13,
            height_variation: 0.4,
            width: 0.008,
            bend: 0.3,
            lush_color: [0.09, 0.11, 0.04],
            dry_color: [0.25, 0.14, 0.07],
            dryness: 0.45,
            // Small cushions, russet patches and russet tips.
            clump_size: 0.45,
            clump_pull: 0.25,
            clump_fan: 0.6,
            clump_height: 0.4,
            clump_color: 0.12,
            droop: 0.15,
            gloss: 0.15,
            tip_tint: [1.35, 1.0, 0.8],
            ..Default::default()
        }
    }

    /// Marsh: tall, upright, dark sedge and reed.
    pub fn marsh() -> Self {
        Self {
            blades_per_m2: 420.0,
            height: 0.8,
            height_variation: 0.35,
            width: 0.014,
            bend: 0.18,
            lush_color: [0.05, 0.11, 0.035],
            dry_color: [0.22, 0.2, 0.1],
            dryness: 0.2,
            // Broad stands of upright, glossy blades.
            clump_size: 1.2,
            clump_pull: 0.2,
            clump_fan: 0.25,
            clump_height: 0.3,
            clump_color: 0.05,
            droop: 0.15,
            gloss: 0.5,
            tip_tint: [1.15, 1.15, 0.8],
            ..Default::default()
        }
    }

    /// Another world's grass: violet and teal blades, tall and upright under low gravity.
    pub fn alien() -> Self {
        Self {
            blades_per_m2: 380.0,
            height: 0.6,
            height_variation: 0.5,
            width: 0.012,
            bend: 0.2,
            lush_color: [0.12, 0.05, 0.16],
            dry_color: [0.05, 0.16, 0.16],
            dryness: 0.3,
            // Low gravity: hardly any droop. Tips flushed pink.
            clump_size: 0.8,
            clump_pull: 0.25,
            clump_fan: 0.6,
            clump_height: 0.4,
            clump_color: 0.1,
            droop: 0.05,
            gloss: 0.55,
            tip_tint: [1.5, 0.9, 1.3],
            ..Default::default()
        }
    }

    /// A ground albedo (linear RGB) that matches the meadow from afar: give it to the terrain
    /// under the field so the ground past the draw distance and between thinned-out blades
    /// reads as grass rather than bare soil.
    pub fn ground_color(&self) -> [f32; 3] {
        let d = self.dryness.clamp(0.0, 1.0);
        std::array::from_fn(|i| {
            (self.lush_color[i] + (self.dry_color[i] - self.lush_color[i]) * d) * 0.85
        })
    }
}

/// Pushes grass aside and flattens it: a character's feet or body, a rolling boulder, a
/// landing ship. A sphere of `radius` meters around the entity; the 16 nearest the camera act.
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[require(Transform)]
#[serde(default)]
pub struct GrassBender {
    pub radius: f32,
}

impl Default for GrassBender {
    fn default() -> Self {
        Self { radius: 0.4 }
    }
}

fn scene_source() -> String {
    [
        include_str!("shaders/grass_common.wgsl"),
        include_str!("shaders/grass.wgsl"),
    ]
    .join("\n")
}

fn cull_source() -> String {
    [
        include_str!("shaders/grass_common.wgsl"),
        include_str!("shaders/grass_cull.wgsl"),
    ]
    .join("\n")
}

fn shadow_source() -> String {
    [
        include_str!("shaders/grass_common.wgsl"),
        include_str!("shaders/grass_shadow.wgsl"),
    ]
    .join("\n")
}

/// Fraction of blades drawn at distance `d` (must match `grass_lod` in the shader): all of them
/// out to `full`, then falling with the square of the distance, as the ground a pixel covers
/// grows, so the blades a pixel holds stay about the same out to the draw distance (Ghost of
/// Tsushima's grass and Unreal's foliage cull by screen size the same way); faded out over the
/// last 30% of `far`.
fn lod_factor(d: f32, full: f32, far: f32) -> f32 {
    let r = full / d.max(1e-3);
    let thin = (r * r).min(1.0);
    let t = ((d - 0.7 * far) / (0.3 * far)).clamp(0.0, 1.0);
    thin * (1.0 - t * t * (3.0 - 2.0 * t))
}

/// How much taller a flowering stem stands than its kind's blades (must match `grass_seed`).
const BLOOM_RISE: f32 = 1.15;

/// Full density out to this distance. Past it the survivors widen, and seen at a glancing
/// angle a few meters of meadow still cover the ground.
fn full_density_distance(draw_distance: f32) -> f32 {
    (draw_distance * 0.15).clamp(4.0, 12.0)
}

/// Of the blades drawn, the fraction that also cast shadows at distance `d` (must match
/// `grass_pass_keep` in the shadow shader): all of them out to 40% of the shadow distance,
/// fading out by the shadow distance, so the meadow's shadows thin out instead of ending at a
/// line.
fn shadow_keep(d: f32, shadow_distance: f32) -> f32 {
    let t = ((d - 0.4 * shadow_distance) / (0.6 * shadow_distance).max(1e-3)).clamp(0.0, 1.0);
    1.0 - t * t * (3.0 - 2.0 * t)
}

/// Quads per blade at distance `d`: enough near the camera for a smooth arch (Ghost of
/// Tsushima draws 7 near and 3 far).
fn segments_at(d: f32) -> u32 {
    match d {
        d if d < 5.0 => 6,
        d if d < 12.0 => 4,
        d if d < 24.0 => 3,
        d if d < 40.0 => 2,
        _ => 1,
    }
}

/// Quads per blade in the shadow maps, where the arch's curve hardly shows.
fn shadow_segments_at(d: f32) -> u32 {
    if d < 8.0 { 2 } else { 1 }
}

/// The draw group of cascade `cascade`'s blades of `segments` (1 or 2) quads, after the
/// screen's six (`grass_emit` in grass_cull.wgsl).
fn shadow_group(cascade: usize, segments: u32) -> usize {
    SCREEN_GROUPS + cascade * SHADOW_SEGMENTS + segments.clamp(1, SHADOW_SEGMENTS as u32) as usize
        - 1
}

/// Quads per blade in draw group `g`.
fn group_segments(g: usize) -> u32 {
    if g < SCREEN_GROUPS {
        g as u32 + 1
    } else {
        ((g - SCREEN_GROUPS) % SHADOW_SEGMENTS) as u32 + 1
    }
}

/// Cull jobs for `count` blades from instance `first`, 64 a job: first and end instance, what
/// for (`mode`: bit 0 the screen, bits 1-4 the cascades) and the segments to draw them with.
fn push_jobs(jobs: &mut Vec<[u32; 4]>, first: u32, count: u32, mode: u32, segments: u32) {
    for start in (0..count).step_by(CULL_CHUNK as usize) {
        jobs.push([
            first + start,
            first + (start + CULL_CHUNK).min(count),
            mode,
            segments,
        ]);
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct GpuTile {
    corner: [f32; 2],
    field: u32,
    segments: u32,
    blades: u32,
    _pad: [u32; 3],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct GpuField {
    origin: [f32; 4],
    axis_x: [f32; 4],
    axis_y: [f32; 4],
    axis_z: [f32; 4],
    grid: [u32; 4],
    shape: [f32; 4],
    lush: [f32; 4],
    dry: [f32; 4],
    lod: [f32; 4],
    clump: [f32; 4],
    look: [f32; 4],
    tip: [f32; 4],
    wear: [f32; 4],
    gust: [f32; 4],
    /// Flower heads: albedo (linear), w: share of blades flowering.
    bloom: [f32; 4],
    /// x: flower head width (m), y: this kind's blades as a share of the field's most.
    kind: [f32; 4],
}

impl GpuField {
    /// Set what `style` decides (shape, colors, clumps, flowers), leaving the field's own
    /// placement, distances and wind as they are.
    fn set_style(&mut self, style: &GrassStyle, share: f32) {
        self.shape = [
            style.height.max(0.0),
            style.height_variation.clamp(0.0, 0.95),
            style.width.max(0.0),
            style.bend.clamp(0.0, 2.0),
        ];
        self.lush = [
            style.lush_color[0],
            style.lush_color[1],
            style.lush_color[2],
            style.leaf.clamp(0.0, 1.0),
        ];
        self.dry = [
            style.dry_color[0],
            style.dry_color[1],
            style.dry_color[2],
            style.wind_response.max(0.0),
        ];
        self.lod[2] = style.lying.clamp(0.0, 1.0);
        self.clump = [
            style.clump_size.max(0.0),
            style.clump_pull.clamp(0.0, 0.95),
            style.clump_fan.clamp(0.0, 1.0),
            style.clump_height.clamp(0.0, 0.9),
        ];
        self.look[0] = style.clump_color.clamp(0.0, 0.5);
        self.look[1] = style.droop.clamp(0.0, 1.0);
        self.look[2] = style.gloss.clamp(0.0, 1.0);
        self.tip = [
            style.tip_tint[0].max(0.0),
            style.tip_tint[1].max(0.0),
            style.tip_tint[2].max(0.0),
            style.stalk.clamp(0.0, 0.8),
        ];
        self.bloom = [
            style.bloom_color[0],
            style.bloom_color[1],
            style.bloom_color[2],
            style.bloom.clamp(0.0, 1.0),
        ];
        self.kind = [style.bloom_size.max(0.0), share.clamp(0.0, 1.0), 0.0, 0.0];
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct GpuBenders {
    count: [u32; 4],
    spheres: [[f32; 4]; MAX_BENDERS],
}

/// One field's map on the GPU and what culling needs to know about it.
struct FieldMap {
    group: wgpu::BindGroup,
    /// This field's chunks in the cull pass's job list: first, count (`grass_cull` in
    /// grass_cull.wgsl).
    cull: wgpu::Buffer,
    resolution: [u32; 2],
    tiles: [u32; 2],
    /// Per tile: lowest and highest ground, highest density.
    tile_bounds: Vec<[f32; 3]>,
    view: wgpu::TextureView,
    _texture: wgpu::Texture,
    /// The worn ground's map, if any, and its placement for `GpuField::wear`.
    worn: Option<(wgpu::Texture, [f32; 4])>,
}

impl FieldMap {
    fn new(
        gpu: &GpuShared,
        layout: &wgpu::BindGroupLayout,
        field: &GrassField,
        no_wear: &wgpu::TextureView,
    ) -> Self {
        let [mut rx, mut rz] = field.resolution;
        let n = (rx as usize) * (rz as usize);
        let usable = |v: &Vec<f32>| v.len() == n && n > 0;
        if !(usable(&field.heights) || usable(&field.density) || usable(&field.dryness)) {
            rx = 2;
            rz = 2;
        }
        rx = rx.max(2);
        rz = rz.max(2);
        let n = (rx * rz) as usize;
        let get = |v: &Vec<f32>, i: usize, default: f32| {
            if v.len() == n { v[i] } else { default }
        };
        let last_kind = field.styles.len().min(MAX_GRASS_KINDS - 1) as u8;
        let texels: Vec<[f32; 4]> = (0..n)
            .map(|i| {
                [
                    get(&field.heights, i, 0.0),
                    get(&field.density, i, 1.0).clamp(0.0, 1.0),
                    get(&field.dryness, i, field.style.dryness).clamp(0.0, 1.0),
                    if field.kinds.len() == n {
                        field.kinds[i].min(last_kind) as f32
                    } else {
                        0.0
                    },
                ]
            })
            .collect();
        let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("grass map"),
            size: wgpu::Extent3d {
                width: rx,
                height: rz,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba32Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        gpu.queue.write_texture(
            texture.as_image_copy(),
            bytemuck::cast_slice(&texels),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(rx * 16),
                rows_per_image: Some(rz),
            },
            wgpu::Extent3d {
                width: rx,
                height: rz,
                depth_or_array_layers: 1,
            },
        );
        let view = texture.create_view(&Default::default());
        let cull = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("grass cull"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let group = map_group(gpu, layout, &view, no_wear, &cull);

        // Tile bounds from the grid points that fall in or next to each tile.
        let size = [field.size[0].max(0.01), field.size[1].max(0.01)];
        let tiles = [
            (size[0] / TILE).ceil().max(1.0) as u32,
            (size[1] / TILE).ceil().max(1.0) as u32,
        ];
        let point_range = |t: u32, axis: usize, res: u32| {
            let to_grid = (res - 1) as f32 / size[axis];
            let lo = ((t as f32 * TILE) * to_grid).floor().max(0.0) as u32;
            let hi = (((t + 1) as f32 * TILE) * to_grid)
                .ceil()
                .min((res - 1) as f32) as u32;
            lo..=hi.max(lo)
        };
        let mut tile_bounds = Vec::with_capacity((tiles[0] * tiles[1]) as usize);
        for tz in 0..tiles[1] {
            for tx in 0..tiles[0] {
                let mut b = [f32::MAX, f32::MIN, 0.0f32];
                for j in point_range(tz, 1, rz) {
                    for i in point_range(tx, 0, rx) {
                        let t = texels[(j * rx + i) as usize];
                        b[0] = b[0].min(t[0]);
                        b[1] = b[1].max(t[0]);
                        b[2] = b[2].max(t[1]);
                    }
                }
                tile_bounds.push(b);
            }
        }
        Self {
            group,
            cull,
            resolution: [rx, rz],
            tiles,
            tile_bounds,
            view,
            _texture: texture,
            worn: None,
        }
    }

    /// Bind `wear` (or none) as this field's worn ground.
    fn set_wear(
        &mut self,
        gpu: &GpuShared,
        layout: &wgpu::BindGroupLayout,
        wear: Option<&GrassWear>,
        no_wear: &wgpu::TextureView,
    ) {
        self.worn = wear.and_then(|w| {
            let [rx, rz] = w.resolution;
            if rx == 0 || rz == 0 || w.wear.len() != (rx * rz) as usize || w.cell <= 0.0 {
                return None;
            }
            let texels: Vec<u8> = w
                .wear
                .iter()
                .map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8)
                .collect();
            let texture = wear_texture(gpu, [rx, rz], &texels);
            Some((texture, [w.origin[0], w.origin[1], 1.0 / w.cell, 1.0]))
        });
        let view = self
            .worn
            .as_ref()
            .map(|(t, _)| t.create_view(&Default::default()));
        self.group = map_group(
            gpu,
            layout,
            &self.view,
            view.as_ref().unwrap_or(no_wear),
            &self.cull,
        );
    }

    /// Placement of the worn ground's map for `GpuField::wear` (w = 0: none).
    fn wear_params(&self) -> [f32; 4] {
        self.worn.as_ref().map_or([0.0; 4], |(_, p)| *p)
    }
}

fn map_group(
    gpu: &GpuShared,
    layout: &wgpu::BindGroupLayout,
    map: &wgpu::TextureView,
    wear: &wgpu::TextureView,
    cull: &wgpu::Buffer,
) -> wgpu::BindGroup {
    gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("grass map"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(map),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(wear),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: cull.as_entire_binding(),
            },
        ],
    })
}

fn entry(binding: u32, buffer: &wgpu::Buffer) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry {
        binding,
        resource: buffer.as_entire_binding(),
    }
}

/// A worn-ground map: one byte per point, 0 untouched .. 255 bare.
fn wear_texture(gpu: &GpuShared, size: [u32; 2], texels: &[u8]) -> wgpu::Texture {
    let extent = wgpu::Extent3d {
        width: size[0],
        height: size[1],
        depth_or_array_layers: 1,
    };
    let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("grass wear"),
        size: extent,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::R8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    gpu.queue.write_texture(
        texture.as_image_copy(),
        texels,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(size[0]),
            rows_per_image: Some(size[1]),
        },
        extent,
    );
    texture
}

/// Draws every [`GrassField`]. Added by `RenderPlugin`.
#[derive(Default)]
pub struct GrassFeature {
    cull_layout: Option<wgpu::BindGroupLayout>,
    records_layout: Option<wgpu::BindGroupLayout>,
    cull_pipeline: Option<wgpu::ComputePipeline>,
    /// Chunks of blades for the cull pass, field by field (first and end instance).
    jobs_buffer: Option<GrowBuffer>,
    /// The blades the cull pass keeps, one `GrassRecord` each, by segment group.
    records_buffer: Option<GrowBuffer>,
    /// The kept blades' indirect draws, one per segment group.
    args_buffer: Option<wgpu::Buffer>,
    cull_group: Option<wgpu::BindGroup>,
    records_group: Option<wgpu::BindGroup>,
    jobs: Vec<[u32; 4]>,
    /// Per field drawn (`order`): its jobs, first and count.
    field_jobs: Vec<(usize, u32, u32)>,
    /// Draw groups with blades this frame.
    groups: [bool; GROUPS],
    map_layout: Option<wgpu::BindGroupLayout>,
    pipeline: Option<wgpu::RenderPipeline>,
    prepass_pipeline: Option<wgpu::RenderPipeline>,
    shadow_pipeline: Option<wgpu::RenderPipeline>,
    tiles_buffer: Option<GrowBuffer>,
    fields_buffer: Option<GrowBuffer>,
    benders_buffer: Option<wgpu::Buffer>,
    maps: HashMap<Entity, FieldMap>,
    /// Bound as the worn ground of fields that have none (one untouched point).
    no_wear: Option<(wgpu::Texture, wgpu::TextureView)>,
    /// Fields drawn this frame, in `GpuField` order.
    order: Vec<Entity>,
    tiles: Vec<GpuTile>,
    fields: Vec<GpuField>,
    stats: FeatureStats,
    wind: SteadyWind,
    /// Made in `prepare_serial`, read in `prepare_shared`.
    fields_query: Option<FieldsQuery>,
    benders_query: Option<QueryState<(&'static GrassBender, &'static GlobalTransform)>>,
    /// Thinned this frame to fit the records binding (warned once per stretch).
    thinned_to_fit: bool,
}

type FieldsQuery = QueryState<(
    Entity,
    Ref<'static, GrassField>,
    &'static GlobalTransform,
    Option<Ref<'static, GrassWear>>,
)>;

/// The wind the blades sway in, kept steady. The frame's wind (sampled where the camera is,
/// so it changes with every gust and step) is eased toward over about a second, and the
/// distance the air has moved (its run) is added up frame by frame. Every gust front, flutter
/// and ripple moves with that run (`grass_wind`), so a change in wind speeds them up or slows
/// them down from where they are. Phases worked out as `time x frequency(speed)` instead jump
/// by the whole elapsed time times the change, every frame the wind changes, which is what
/// made meadows shake (playtest #16).
#[derive(Default)]
struct SteadyWind {
    wind: Vec3,
    run: f64,
    time: Option<f32>,
}

impl SteadyWind {
    /// Seconds the blades take to follow a change in the wind (63 %).
    const EASE_S: f32 = 0.8;
    /// The run wraps after this many meters (one pattern jump every few hours of wind).
    const WRAP_M: f64 = 65536.0;

    /// Moves on to frame `time` (seconds, the frame's clock), `dt` seconds after the last.
    fn update(&mut self, wind: Vec3, time: f32, dt: f32) {
        match self.time {
            Some(last) if last == time => return,
            Some(_) => {
                let dt = dt.clamp(0.0, 0.25);
                let k = 1.0 - (-dt / Self::EASE_S).exp();
                self.wind += (wind - self.wind) * k;
                self.run = (self.run + self.wind.length() as f64 * dt as f64) % Self::WRAP_M;
            }
            None => self.wind = wind,
        }
        self.time = Some(time);
    }

    /// The field's `gust`: the steady wind (xyz) and the run less speed x `time`, so the
    /// shader's run at any time t near this frame is `w + |xyz| t` (also for the previous
    /// frame's positions in the motion vectors).
    fn uniform(&self, time: f32) -> [f32; 4] {
        let w = self.wind;
        let offset = self.run - w.length() as f64 * time as f64;
        [w.x, w.y, w.z, offset as f32]
    }
}

impl GrassFeature {
    /// The blades the cull pass kept in draw `groups`: one indirect draw each.
    fn draw_kept(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        pipeline: &wgpu::RenderPipeline,
        groups: std::ops::Range<usize>,
    ) {
        let (Some(group), Some(args)) = (&self.records_group, &self.args_buffer) else {
            return;
        };
        if self.field_jobs.is_empty() {
            return;
        }
        pass.set_pipeline(pipeline);
        pass.set_bind_group(1, group, &[]);
        for g in groups.filter(|&g| self.groups[g]) {
            pass.draw_indirect(args, g as u64 * 16);
        }
    }
}

impl RenderFeature for GrassFeature {
    fn name(&self) -> &str {
        "grass"
    }

    fn order(&self) -> i32 {
        10
    }

    fn init(&mut self, gpu: &GpuShared) {
        use wgpu::ShaderStages as S;
        let device = &gpu.device;
        let texture = |binding| {
            layout_entry(
                binding,
                S::VERTEX | S::COMPUTE,
                wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
            )
        };
        let map_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("grass map"),
            entries: &[
                texture(0),
                texture(1),
                layout_entry(
                    2,
                    S::COMPUTE,
                    wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                ),
            ],
        });
        let buffer = |binding, stages, ty| {
            layout_entry(
                binding,
                stages,
                wgpu::BindingType::Buffer {
                    ty,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
            )
        };
        let read = wgpu::BufferBindingType::Storage { read_only: true };
        let write = wgpu::BufferBindingType::Storage { read_only: false };
        let cull_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("grass cull"),
            entries: &[
                buffer(0, S::COMPUTE, read),
                buffer(1, S::COMPUTE, read),
                buffer(2, S::COMPUTE, wgpu::BufferBindingType::Uniform),
                buffer(3, S::COMPUTE, read),
                buffer(4, S::COMPUTE, write),
                buffer(5, S::COMPUTE, write),
            ],
        });
        let records_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("grass records"),
            entries: &[buffer(1, S::VERTEX, read), buffer(4, S::VERTEX, read)],
        });
        let cull_shader = gpu.shader("grass cull", &cull_source(), true);
        let cull_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("grass cull"),
            bind_group_layouts: &[
                Some(&gpu.frame_layout),
                Some(&cull_layout),
                Some(&map_layout),
            ],
            immediate_size: 0,
        });
        self.cull_pipeline = Some(device.create_compute_pipeline(
            &wgpu::ComputePipelineDescriptor {
                label: Some("grass cull"),
                layout: Some(&cull_pipeline_layout),
                module: &cull_shader,
                entry_point: Some("cs_cull"),
                compilation_options: Default::default(),
                cache: None,
            },
        ));
        let shader = gpu.shader("grass", &scene_source(), true);
        let shadow_shader = gpu.shader("grass shadow", &shadow_source(), false);
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("grass"),
            bind_group_layouts: &[Some(&gpu.frame_layout), Some(&records_layout)],
            immediate_size: 0,
        });
        let shadow_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("grass shadow"),
            bind_group_layouts: &[Some(&gpu.shadow_view_layout), Some(&records_layout)],
            immediate_size: 0,
        });
        let primitive = wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleStrip,
            cull_mode: None,
            ..Default::default()
        };
        let scene_pipeline = |label, vs, fs, targets: &[Option<wgpu::ColorTargetState>]| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some(vs),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive,
                depth_stencil: Some(gpu.depth_state(true)),
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(fs),
                    compilation_options: Default::default(),
                    targets,
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        self.pipeline = Some(scene_pipeline(
            "grass",
            "vs_main",
            "fs_main",
            &[Some(wgpu::ColorTargetState {
                format: gpu.formats.hdr,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        ));
        self.prepass_pipeline = Some(scene_pipeline(
            "grass prepass",
            "vs_prepass",
            "fs_prepass",
            &gpu.prepass_targets(),
        ));
        self.shadow_pipeline = Some(device.create_render_pipeline(
            &wgpu::RenderPipelineDescriptor {
                label: Some("grass shadow"),
                layout: Some(&shadow_layout),
                vertex: wgpu::VertexState {
                    module: &shadow_shader,
                    entry_point: Some("vs_shadow"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: wgpu::PrimitiveState {
                    unclipped_depth: gpu.features.contains(wgpu::Features::DEPTH_CLIP_CONTROL),
                    ..primitive
                },
                depth_stencil: Some(gpu.shadow_depth_state()),
                multisample: Default::default(),
                fragment: None,
                multiview_mask: None,
                cache: None,
            },
        ));
        self.tiles_buffer = Some(GrowBuffer::new(
            device,
            "grass tiles",
            wgpu::BufferUsages::STORAGE,
            256 * std::mem::size_of::<GpuTile>() as u64,
        ));
        self.fields_buffer = Some(GrowBuffer::new(
            device,
            "grass fields",
            wgpu::BufferUsages::STORAGE,
            4 * std::mem::size_of::<GpuField>() as u64,
        ));
        self.jobs_buffer = Some(GrowBuffer::new(
            device,
            "grass jobs",
            wgpu::BufferUsages::STORAGE,
            1024 * 8,
        ));
        self.records_buffer = Some(GrowBuffer::new(
            device,
            "grass records",
            wgpu::BufferUsages::STORAGE,
            4096 * RECORD_BYTES,
        ));
        self.args_buffer = Some(device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("grass draws"),
            size: (GROUPS * 16) as u64,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::INDIRECT
                | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        }));
        self.cull_layout = Some(cull_layout);
        self.records_layout = Some(records_layout);
        self.benders_buffer = Some(device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("grass benders"),
            size: std::mem::size_of::<GpuBenders>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        }));
        self.map_layout = Some(map_layout);
        let none = wear_texture(gpu, [1, 1], &[0]);
        let view = none.create_view(&Default::default());
        self.no_wear = Some((none, view));
    }

    fn prepares_shared(&self) -> bool {
        true
    }

    fn prepare_serial(
        &mut self,
        world: &mut World,
        _gpu: &GpuShared,
        _view: &ViewData,
        _settings: &RenderSettings,
    ) {
        if self.fields_query.is_none() {
            self.fields_query = Some(world.query());
        }
        if self.benders_query.is_none() {
            self.benders_query = Some(world.query());
        }
    }

    fn prepare_shared(
        &mut self,
        world: &World,
        gpu: &GpuShared,
        view: &ViewData,
        settings: &RenderSettings,
        _out: &mut PrepareOut,
    ) {
        self.order.clear();
        self.tiles.clear();
        self.fields.clear();
        self.jobs.clear();
        self.field_jobs.clear();
        self.groups = [false; GROUPS];
        let mut group_blades = [0u64; GROUPS];
        self.stats = FeatureStats::default();
        let (Some(map_layout), Some((_, no_wear))) = (&self.map_layout, &self.no_wear) else {
            return;
        };

        let scale = settings.grass_density.max(0.0);
        let cascades = if view.sun_shadows {
            (view.cascades.count as usize).min(MAX_CASCADES)
        } else {
            0
        };
        // Snow on the ground by the camera, for the shadow pass (which can't read the frame's
        // surface weather; the scene pass reads it under every blade).
        let snow_here = world
            .get_resource::<engine_world::SurfaceLayers>()
            .map_or(0.0, |l| l.grid.sample(view.origin).snow_m.max(0.0));
        if !view.extra_view {
            self.wind.update(view.wind, view.time, view.delta);
        }
        let gust = self.wind.uniform(view.time);
        let mut seen = Vec::new();
        let (Some(fields_query), Some(benders_query)) =
            (&mut self.fields_query, &mut self.benders_query)
        else {
            return;
        };
        for (entity, field, gt, wear) in fields_query.iter(world) {
            seen.push(entity);
            let fresh = field.is_changed() || !self.maps.contains_key(&entity);
            if fresh {
                self.maps
                    .insert(entity, FieldMap::new(gpu, map_layout, &field, no_wear));
            }
            let map = self.maps.get_mut(&entity).expect("inserted above");
            let rewear = match &wear {
                Some(w) => fresh || w.is_changed(),
                None => map.worn.is_some(),
            };
            if rewear {
                map.set_wear(gpu, map_layout, wear.as_deref(), no_wear);
            }
            let map = &*map;
            let style = &field.style;
            let most_per_m2 = field
                .palette()
                .map(|s| s.blades_per_m2.max(0.0))
                .fold(0.0f32, f32::max);
            let per_m2 = (most_per_m2 * scale).clamp(0.0, MAX_BLADES_PER_M2);
            let far = style.draw_distance.max(1.0);
            if per_m2 <= 0.0 || field.size[0] <= 0.0 || field.size[1] <= 0.0 {
                continue;
            }
            let full = full_density_distance(far);
            let max_blades = ((per_m2 * TILE * TILE).ceil() as u32).clamp(1, MAX_BLADES_PER_TILE);
            let rotation = gt.rotation();
            let [ax, ay, az] = [Vec3::X, Vec3::Y, Vec3::Z].map(|a| rotation * a);
            let half = DVec3::new(field.size[0] as f64 * 0.5, 0.0, field.size[1] as f64 * 0.5);
            let corner = gt.translation() - rotation.as_dquat() * half;
            let origin = view.relative(corner);
            // The camera in field-local coordinates.
            let cam = rotation.inverse() * -origin;
            let blade_top = field
                .palette()
                .map(|s| s.height * (1.0 + s.height_variation.max(0.0)) * BLOOM_RISE)
                .fold(0.0f32, f32::max);

            let field_index = self.order.len();
            // The field's first entry in the fields buffer: one for each kind it grows.
            let gpu_index = self.fields.len() as u32;
            let first_job = self.jobs.len() as u32;
            let mut drew = false;
            let tx0 = ((cam.x - far) / TILE).floor().max(0.0) as u32;
            let tz0 = ((cam.z - far) / TILE).floor().max(0.0) as u32;
            let tx1 = ((cam.x + far) / TILE).floor().max(-1.0) as i64;
            let tz1 = ((cam.z + far) / TILE).floor().max(-1.0) as i64;
            let tx1 = tx1.min(map.tiles[0] as i64 - 1);
            let tz1 = tz1.min(map.tiles[1] as i64 - 1);
            for tz in tz0 as i64..=tz1 {
                for tx in tx0 as i64..=tx1 {
                    let b = map.tile_bounds[(tz * map.tiles[0] as i64 + tx) as usize];
                    if b[2] <= 0.0 {
                        continue;
                    }
                    let lo = Vec3::new(tx as f32 * TILE, b[0], tz as f32 * TILE);
                    let hi = Vec3::new(
                        ((tx + 1) as f32 * TILE).min(field.size[0]),
                        b[1] + blade_top,
                        ((tz + 1) as f32 * TILE).min(field.size[1]),
                    );
                    let d = (cam.clamp(lo, hi) - cam).length();
                    if d >= far {
                        continue;
                    }
                    let count = ((max_blades as f32) * lod_factor(d, full, far) * b[2])
                        .ceil()
                        .min(max_blades as f32) as u32;
                    if count == 0 {
                        continue;
                    }
                    let center_local = (lo + hi) * 0.5;
                    let center =
                        origin + ax * center_local.x + ay * center_local.y + az * center_local.z;
                    let radius = ((hi - lo) * 0.5).length();
                    let visible = view.frustum.intersects_sphere(center, radius);
                    let shadows = d < style.shadow_distance;
                    let in_cascade: Vec<usize> = if shadows {
                        (0..cascades)
                            .filter(|&c| view.cascades.sphere_in_cascade(c, center, radius))
                            .collect()
                    } else {
                        Vec::new()
                    };
                    if !visible && in_cascade.is_empty() {
                        continue;
                    }
                    let tile_index = self.tiles.len() as u32;
                    if tile_index >= 1 << (32 - BLADE_BITS) {
                        continue;
                    }
                    let segments = segments_at(d);
                    self.tiles.push(GpuTile {
                        corner: [lo.x, lo.z],
                        field: gpu_index,
                        segments,
                        blades: max_blades,
                        _pad: [0; 3],
                    });
                    // The cull pass grows these blades, in chunks: for the screen, and
                    // (fewer, further out) for the cascades they cast into.
                    let first = tile_index << BLADE_BITS;
                    if visible {
                        push_jobs(&mut self.jobs, first, count, 1, segments);
                        group_blades[segments as usize - 1] += count as u64;
                        self.stats.instances += count;
                        self.stats.triangles += count as u64 * (2 * segments as u64 - 1);
                    }
                    let shadow_count = ((max_blades as f32)
                        * lod_factor(d, full, far)
                        * shadow_keep(d, style.shadow_distance)
                        * b[2])
                        .ceil()
                        .min(count as f32) as u32;
                    if shadow_count > 0 && !in_cascade.is_empty() {
                        let segments = shadow_segments_at(d);
                        let mask = in_cascade.iter().fold(0, |m, &c| m | 2 << c);
                        push_jobs(&mut self.jobs, first, shadow_count, mask, segments);
                        for c in in_cascade {
                            group_blades[shadow_group(c, segments)] += shadow_count as u64;
                        }
                    }
                    drew = true;
                }
            }
            if !drew {
                continue;
            }
            let jobs = self.jobs.len() as u32 - first_job;
            if jobs > 0 {
                self.field_jobs.push((field_index, first_job, jobs));
            }
            let seed = (entity.to_bits() as u32)
                ^ ((entity.to_bits() >> 32) as u32).wrapping_mul(0x9e37_79b9);
            self.order.push(entity);
            let kinds = field.palette().count() as u32;
            let mut base = GpuField {
                origin: origin.extend(0.0).to_array(),
                axis_x: ax.extend(field.size[0]).to_array(),
                axis_y: ay.extend(field.size[1]).to_array(),
                axis_z: az.extend(0.0).to_array(),
                grid: [map.resolution[0], map.resolution[1], kinds, seed],
                lod: [full, far, 0.0, style.shadow_distance.max(0.0)],
                look: [0.0, 0.0, 0.0, snow_here],
                wear: map.wear_params(),
                gust,
                ..GpuField::zeroed()
            };
            // One entry for each kind: the same field, each kind's look.
            for kind in field.palette() {
                let share = if most_per_m2 > 0.0 {
                    kind.blades_per_m2.max(0.0) / most_per_m2
                } else {
                    1.0
                };
                base.set_style(kind, share);
                self.fields.push(base);
            }
        }
        self.maps.retain(|e, _| seen.contains(e));

        // Benders nearest the camera.
        let mut benders: Vec<[f32; 4]> = benders_query
            .iter(world)
            .map(|(b, gt)| {
                view.relative(gt.translation())
                    .extend(b.radius.max(0.0))
                    .to_array()
            })
            .filter(|s| s[3] > 0.0)
            .collect();
        benders.sort_by(|a, b| {
            let d = |s: &[f32; 4]| s[0] * s[0] + s[1] * s[1] + s[2] * s[2];
            d(a).total_cmp(&d(b))
        });
        benders.truncate(MAX_BENDERS);
        let mut gpu_benders = GpuBenders::zeroed();
        gpu_benders.count[0] = benders.len() as u32;
        gpu_benders.spheres[..benders.len()].copy_from_slice(&benders);

        // Upload.
        if self.order.is_empty() {
            return;
        }
        let device = &gpu.device;
        let queue = &gpu.queue;
        let (Some(tiles), Some(fields), Some(benders_buffer)) = (
            &mut self.tiles_buffer,
            &mut self.fields_buffer,
            &self.benders_buffer,
        ) else {
            return;
        };
        let grown_tiles = tiles.write(device, queue, bytemuck::cast_slice(&self.tiles));
        let grown_fields = fields.write(device, queue, bytemuck::cast_slice(&self.fields));
        queue.write_buffer(benders_buffer, 0, bytemuck::bytes_of(&gpu_benders));

        // The cull pass: its jobs, each field's share of them, and the six draws it fills in,
        // each group's records after the last's (a draw's first vertex is 16 x its first
        // record: grass.wgsl).
        // More blades than one binding of records holds (a wide meadow at a long draw distance
        // on a GPU with small bindings): thin every chunk alike rather than drop the meadow. A
        // chunk keeps its first blades, as distance thinning does, and blades are placed by
        // hashing their index, so what stays is an even sample.
        let limit = device.limits().max_storage_buffer_binding_size;
        let budget = (limit / 2 / RECORD_BYTES).min(u32::MAX as u64 / 16);
        let wanted: u64 = group_blades.iter().sum();
        if wanted > budget {
            let keep = budget as f64 / wanted as f64 * 0.98;
            group_blades = [0; GROUPS];
            self.stats.instances = 0;
            self.stats.triangles = 0;
            for job in &mut self.jobs {
                let kept = ((job[1] - job[0]) as f64 * keep) as u32;
                job[1] = job[0] + kept;
                let (mode, segments) = (job[2], job[3]);
                if mode == 1 {
                    group_blades[segments as usize - 1] += kept as u64;
                    self.stats.instances += kept;
                    self.stats.triangles += kept as u64 * (2 * segments as u64 - 1);
                } else {
                    for c in 0..MAX_CASCADES {
                        if mode & (2 << c) != 0 {
                            group_blades[shadow_group(c, segments)] += kept as u64;
                        }
                    }
                }
            }
            if !self.thinned_to_fit {
                tracing::warn!(
                    "grass: {wanted} blades would overflow the {} MB records binding; thinned to {:.0}%",
                    limit / (1 << 20),
                    keep * 100.0
                );
            }
            self.thinned_to_fit = true;
        } else {
            self.thinned_to_fit = false;
        }
        let mut args = [[0u32; 4]; GROUPS];
        let mut records = 0u64;
        for (g, blades) in group_blades.into_iter().enumerate() {
            if blades > 0 {
                args[g] = [2 * group_segments(g) + 1, 0, (records * 16) as u32, 0];
                self.groups[g] = true;
                self.stats.draw_calls += 1;
                records += blades;
            }
        }
        if records * 16 > u32::MAX as u64 || records * RECORD_BYTES > limit / 2 {
            self.groups = [false; GROUPS];
            self.field_jobs.clear();
        }
        for &(field, first, count) in &self.field_jobs {
            if let Some(map) = self.maps.get(&self.order[field]) {
                queue.write_buffer(&map.cull, 0, bytemuck::cast_slice(&[first, count, 0, 0]));
            }
        }
        let (
            Some(jobs),
            Some(records_buffer),
            Some(args_buffer),
            Some(cull_layout),
            Some(records_layout),
        ) = (
            &mut self.jobs_buffer,
            &mut self.records_buffer,
            &self.args_buffer,
            &self.cull_layout,
            &self.records_layout,
        )
        else {
            return;
        };
        queue.write_buffer(args_buffer, 0, bytemuck::cast_slice(&args));
        let grown_jobs =
            !self.jobs.is_empty() && jobs.write(device, queue, bytemuck::cast_slice(&self.jobs));
        let grown_records = records_buffer.reserve(device, records.max(1) * RECORD_BYTES);
        if grown_tiles
            || grown_fields
            || grown_jobs
            || grown_records
            || self.cull_group.is_none()
            || self.records_group.is_none()
        {
            self.cull_group = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("grass cull"),
                layout: cull_layout,
                entries: &[
                    entry(0, &tiles.buffer),
                    entry(1, &fields.buffer),
                    entry(2, benders_buffer),
                    entry(3, &jobs.buffer),
                    entry(4, &records_buffer.buffer),
                    entry(5, args_buffer),
                ],
            }));
            self.records_group = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("grass records"),
                layout: records_layout,
                entries: &[entry(1, &fields.buffer), entry(4, &records_buffer.buffer)],
            }));
        }
    }

    fn compute(&mut self, encoder: &mut wgpu::CommandEncoder, ctx: &FrameContext) {
        let (Some(pipeline), Some(group)) = (&self.cull_pipeline, &self.cull_group) else {
            return;
        };
        if self.field_jobs.is_empty() {
            return;
        }
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("grass cull"),
            timestamp_writes: None,
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, ctx.frame_group, &[]);
        pass.set_bind_group(1, group, &[]);
        for &(field, _, jobs) in &self.field_jobs {
            let Some(map) = self.maps.get(&self.order[field]) else {
                continue;
            };
            pass.set_bind_group(2, &map.group, &[]);
            pass.dispatch_workgroups(jobs.min(MAX_GROUPS_X), jobs.div_ceil(MAX_GROUPS_X), 1);
        }
    }

    fn draw_prepass(&self, pass: &mut wgpu::RenderPass<'_>, _ctx: &FrameContext) {
        if let Some(pipeline) = &self.prepass_pipeline {
            self.draw_kept(pass, pipeline, 0..SCREEN_GROUPS);
        }
    }

    fn draw(&self, phase: RenderPhase, pass: &mut wgpu::RenderPass<'_>, _ctx: &FrameContext) {
        if phase != RenderPhase::Opaque {
            return;
        }
        if let Some(pipeline) = &self.pipeline {
            self.draw_kept(pass, pipeline, 0..SCREEN_GROUPS);
        }
    }

    fn draw_shadow(&self, pass: &mut wgpu::RenderPass<'_>, cascade: usize, _ctx: &FrameContext) {
        if let (Some(pipeline), true) = (&self.shadow_pipeline, cascade < MAX_CASCADES) {
            let first = shadow_group(cascade, 1);
            self.draw_kept(pass, pipeline, first..first + SHADOW_SEGMENTS);
        }
    }

    fn stats(&self) -> FeatureStats {
        self.stats
    }

    fn describe(&self) -> serde_json::Value {
        serde_json::json!({
            "fields": self.order.len(),
            "tiles": self.tiles.len(),
            "cull_jobs": self.jobs.len(),
            "blades": self.stats.instances,
            "draw_groups": self.groups.iter().filter(|g| **g).count(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grass_shaders_compile() {
        use crate::features::{compose_shader, validate_wgsl};
        validate_wgsl("grass", &compose_shader(&scene_source(), true)).unwrap();
        validate_wgsl("grass cull", &compose_shader(&cull_source(), true)).unwrap();
        validate_wgsl("grass shadow", &compose_shader(&shadow_source(), false)).unwrap();
    }

    #[test]
    fn lod_is_full_near_and_gone_at_the_draw_distance() {
        assert_eq!(lod_factor(1.0, 14.0, 70.0), 1.0);
        assert_eq!(lod_factor(14.0, 14.0, 70.0), 1.0);
        let mid = lod_factor(30.0, 14.0, 70.0);
        assert!(mid > 0.2 && mid < 0.5, "{mid}");
        assert_eq!(lod_factor(70.0, 14.0, 70.0), 0.0);
        // Never increases with distance.
        let mut prev = 1.0;
        for i in 0..200 {
            let f = lod_factor(i as f32 * 0.5, 14.0, 70.0);
            assert!(f <= prev + 1e-6);
            prev = f;
        }
    }

    #[test]
    fn gpu_field_matches_the_shader_struct() {
        // Sixteen vec4s, as `GrassField` in grass_common.wgsl.
        assert_eq!(std::mem::size_of::<GpuField>(), 16 * 16);
    }

    /// The run the shader sees (`gust.w + speed x t`) at frame time `t`.
    fn shader_run(w: &SteadyWind, t: f32) -> f32 {
        let g = w.uniform(t);
        g[3] + Vec3::new(g[0], g[1], g[2]).length() * t
    }

    #[test]
    fn gusts_move_the_sway_on_smoothly_however_late_in_the_day() {
        // An hour and a half into play, the wind at the camera changes every frame (gusts,
        // walking about). The sway's phase must move on by about speed x dt a frame, not by the
        // clock times the change (which shook meadows, playtest #16).
        let mut w = SteadyWind::default();
        let dt = 1.0 / 60.0;
        let mut t = 5400.0_f32;
        w.update(Vec3::new(4.0, 0.0, 0.0), t, dt);
        let mut last = shader_run(&w, t);
        let mut eased = Vec::new();
        for i in 0..600 {
            t += dt;
            // Gusty: 2 to 12 m/s over a few seconds, changing direction, jittering each frame.
            let speed = 7.0 + 4.0 * (i as f32 * 0.01).sin() + (i as f32 * 2.3).sin();
            let dir = Vec3::new((i as f32 * 0.002).cos(), 0.0, (i as f32 * 0.002).sin());
            w.update(dir * speed, t, dt);
            // A second view in the same frame changes nothing.
            w.update(dir * 30.0, t, dt);
            let run = shader_run(&w, t);
            let step = run - last;
            assert!(
                step >= -0.02 && step < 12.0 * dt + 0.02,
                "frame {i}: the run stepped {step} m"
            );
            // The previous frame's run, as the motion vectors work it out, is where it was.
            let prev = w.uniform(t)[3] + w.wind.length() * (t - dt);
            assert!(
                (prev - last).abs() < 12.0 * dt + 0.05,
                "frame {i}: {prev} vs {last}"
            );
            last = run;
            eased.push(w.wind.length());
        }
        // The steady wind follows the gusts, without their frame-to-frame jitter.
        let jumps = eased
            .windows(2)
            .map(|p| (p[1] - p[0]).abs())
            .fold(0.0, f32::max);
        assert!(jumps < 0.2, "steady wind jumped {jumps} m/s in a frame");
        assert!(eased.iter().cloned().fold(0.0, f32::max) > 9.0);
    }

    #[test]
    fn biome_presets_grow_clumped_grass_of_their_own() {
        let presets = [
            GrassStyle::meadow(),
            GrassStyle::savanna(),
            GrassStyle::tundra(),
            GrassStyle::marsh(),
            GrassStyle::alien(),
        ];
        for (i, p) in presets.iter().enumerate() {
            assert!(p.height > 0.05 && p.blades_per_m2 > 0.0, "{p:?}");
            assert!(
                p.clump_size > 0.0 && (0.0..1.0).contains(&p.clump_pull),
                "{p:?}"
            );
            assert!(p.ground_color().iter().all(|c| (0.0..1.0).contains(c)));
            for q in &presets[i + 1..] {
                assert_ne!(p, q);
            }
        }
        // Savanna tussocks crowd tighter and stand taller than tundra cushions.
        assert!(GrassStyle::savanna().height > 4.0 * GrassStyle::tundra().height);
        assert!(GrassStyle::alien().droop < GrassStyle::meadow().droop);
    }

    #[test]
    fn sampled_maps_cover_the_field_corner_to_corner() {
        let f = GrassField::flat(10.0, 20.0).sample([3, 5], |x, z| (x, z, 0.0));
        assert_eq!(f.heights.len(), 15);
        assert_eq!((f.heights[0], f.density[0]), (-5.0, -10.0));
        assert_eq!((f.heights[14], f.density[14]), (5.0, 10.0));
        assert_eq!(f.heights[1], 0.0);
    }
}
