//! Tunable render settings. All of these are registered resources, so the AI bridge, console
//! and scripts can read and patch them by name (`resource.set RenderSettings {...}`).

use engine_core::prelude::*;
use engine_world::Scalability;

/// Tonemapping curve.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Tonemapper {
    /// AgX with Blender's "Punchy" look: AgX's graceful highlights with more contrast and
    /// color, closer to a camera's picture (default).
    #[default]
    AgXPunchy,
    /// Filmic, hue-preserving; handles very bright colors gracefully. Flatter than `AgXPunchy`.
    AgX,
    /// Khronos PBR Neutral: keeps base colors as authored up to bright highlights (product
    /// shots, checking materials).
    Neutral,
    /// The classic ACES filmic look (more contrast, saturated highlights).
    Aces,
    Reinhard,
    /// No curve: clamps. For debugging.
    None,
}

/// How edges are smoothed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum AntiAliasing {
    /// Temporal anti-aliasing (default): the camera shifts by a fraction of a pixel every
    /// frame and the results are blended along motion vectors. Smooth edges, stable thin
    /// geometry (grass, wires), and it also cleans up the noise of ambient occlusion and soft
    /// shadows. Needs a few frames after a camera cut to converge.
    #[default]
    Taa,
    /// No anti-aliasing (sharpest, jagged edges).
    None,
}

/// Whether the renderer records and submits on its own thread (`RenderSettings::pipelining`),
/// overlapping a frame's passes with the next frame's simulation. The image shows the world one
/// simulation step late; screenshots (`capture_now`) still render inline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Pipelining {
    /// Always inline.
    Off,
    /// On with a window, inline headless (tests and tools get each frame's picture at once).
    #[default]
    Windowed,
    /// On everywhere, also headless (for tests of the pipelined path).
    Always,
}

impl Pipelining {
    /// Whether frames are pipelined for a headless or windowed app.
    pub fn on(self, headless: bool) -> bool {
        match Self::from_env().unwrap_or(self) {
            Self::Off => false,
            Self::Windowed => !headless,
            Self::Always => true,
        }
    }

    /// `ENGINE_PIPELINING=off|windowed|always`, when set, overrides the setting (to compare the
    /// two on one build).
    fn from_env() -> Option<Self> {
        static MODE: std::sync::OnceLock<Option<Pipelining>> = std::sync::OnceLock::new();
        *MODE.get_or_init(|| {
            match std::env::var("ENGINE_PIPELINING")
                .ok()?
                .to_ascii_lowercase()
                .as_str()
            {
                "off" | "0" => Some(Self::Off),
                "windowed" => Some(Self::Windowed),
                "always" | "on" | "1" => Some(Self::Always),
                other => {
                    tracing::warn!("ENGINE_PIPELINING={other} is not off, windowed or always");
                    None
                }
            }
        })
    }
}

/// When the mesh pipelines' variants are compiled ahead on worker threads
/// (`RenderSettings::pipeline_warmup`), rather than on the frame that first draws with each.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum PipelineWarmup {
    /// Each on first use.
    Off,
    /// All of them with a window; headless apps (tests, tools) compile only what they draw.
    #[default]
    Windowed,
    /// All of them everywhere.
    Always,
}

impl PipelineWarmup {
    pub fn on(self, headless: bool) -> bool {
        match Self::from_env().unwrap_or(self) {
            Self::Off => false,
            Self::Windowed => !headless,
            Self::Always => true,
        }
    }

    /// `ENGINE_PIPELINE_WARMUP=off|windowed|always`, when set, overrides the setting: a
    /// headless profile run warms its pipelines as a windowed game does, so its frames show
    /// what a player's would.
    fn from_env() -> Option<Self> {
        static MODE: std::sync::OnceLock<Option<PipelineWarmup>> = std::sync::OnceLock::new();
        *MODE.get_or_init(|| {
            match std::env::var("ENGINE_PIPELINE_WARMUP")
                .ok()?
                .to_ascii_lowercase()
                .as_str()
            {
                "off" | "0" => Some(Self::Off),
                "windowed" => Some(Self::Windowed),
                "always" | "on" | "1" => Some(Self::Always),
                _ => None,
            }
        })
    }
}

/// Block compression of material images on the GPU (`RenderSettings::texture_compression`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
pub enum TextureCompression {
    /// Images stay RGBA8 (4 bytes a texel).
    Off,
    /// BC1 for opaque color (half a byte a texel; some banding in smooth gradients), BC7 for
    /// the rest, BC4 for one-channel slots.
    Compact,
    /// BC7 for color and data (1 byte a texel, close to the original), BC4 for one-channel
    /// slots such as height and occlusion (half a byte). Default.
    #[default]
    Quality,
}

/// Resolution of the volumetric fog grid (`RenderSettings::volumetric_fog_quality`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
pub enum VolumetricQuality {
    /// 16-pixel tiles, 64 depth slices: soft shafts, cheapest.
    Low,
    /// 8-pixel tiles, 64 depth slices.
    Medium,
    /// 8-pixel tiles, 128 depth slices (default): crisp shafts between trunks and through
    /// windows, thin ground fog.
    #[default]
    High,
}

impl VolumetricQuality {
    /// Tile size in pixels and number of depth slices.
    pub fn tile_and_slices(self) -> (u32, u32) {
        match self {
            VolumetricQuality::Low => (16, 64),
            VolumetricQuality::Medium => (8, 64),
            VolumetricQuality::High => (8, 128),
        }
    }
}

/// Bits of [`RenderSettings::debug_skip`]: parts of the shading left out to measure what each
/// costs on the GPU (lib_frame.wgsl `shade_skip`). A uniform read and a branch in a normal
/// frame.
pub mod shade_skip {
    /// The sun's shadow maps read as lit: cascades, the far map, people's insets.
    pub const SUN_SHADOW_MAPS: u32 = 1;
    /// People's capsule shadows.
    pub const CAPSULES: u32 = 2;
    /// The indoor volumes' loops (sky light and the probes' walls).
    pub const SKY_VOLUMES: u32 = 4;
    /// The eight probes a pixel reads (their tracing still runs).
    pub const PROBE_LOOKUP: u32 = 8;
    /// The reflection's march through the sky occlusion map.
    pub const SPECULAR_SKY: u32 = 16;
    /// The point light loop.
    pub const POINT_LIGHTS: u32 = 32;
    /// Fog and aerial perspective on surfaces.
    pub const SURFACE_FOG: u32 = 64;
    /// All of `shade_surface`: surfaces show their albedo.
    pub const LIGHTING: u32 = 128;
    /// The ground's material textures (their means instead).
    pub const TERRAIN_TEXTURES: u32 = 256;
    /// The ground's fractal noises (their middle instead).
    pub const TERRAIN_NOISE: u32 = 512;
    /// The ground's whole pixel shader (a flat color: what is left is its triangles).
    pub const TERRAIN: u32 = 1024;
    /// Opaque meshes' whole scene pixel shader (a flat color).
    pub const MESH_SCENE: u32 = 2048;
    /// Opaque meshes' material in the prepass (the vertex normal instead).
    pub const MESH_PREPASS_MATERIAL: u32 = 4096;
    /// The sky occlusion map's ring taps (surfaces and fog read open sky; the map is drawn).
    pub const SKY_MAP_TAPS: u32 = 8192;
    /// The screen light pass writes no sun: with [`trial_cut::SCREEN_LIGHT`] on, the pixels
    /// whose shading takes its light from the pass's maps lose their sunlight and the rest
    /// keep it (a picture of where the maps are used). With [`trial_cut::SCREEN_FOG`] on,
    /// they lose their fog and haze the same way.
    pub const SCREEN_LIGHT_SUN: u32 = 16384;
    /// With `trial_cut::GROUND_ONCE`: the ground's pixels colored once a pixel are drawn
    /// magenta and the rest of the ground green (a picture of what the cut reaches).
    pub const SHOW_GROUND_ONCE: u32 = 65536;
    /// With `trial_cut::MESHES_ONCE`: the pixels lit from the stored material are drawn
    /// magenta (a picture of what the cut reaches; meshes left to the old way keep their look).
    pub const SHOW_MESHES_ONCE: u32 = 131072;
    /// Not a skip: count the meshes' vertex runs, triangles and fragment runs in the prepass
    /// and the scene pass of the main view (mesh_counts.rs; `render.stats`, the meshes'
    /// `mesh_runs`). Needs pipeline statistics queries.
    pub const COUNT_MESH_RUNS: u32 = 262144;
}

/// Bits of [`RenderSettings::trial_cuts`]: cheaper ways to draw the same picture, switchable
/// while they are measured (lib_frame.wgsl `trial_cut`).
pub mod trial_cut {
    /// The sky occlusion map read with four taps a ring instead of eight on surfaces, the rings
    /// turned by a different part of a step each pixel and frame (temporal anti-aliasing
    /// averages them).
    pub const SKY_RING_TAPS: u32 = 1;
    /// The ground's material textures filtered with at most 8 anisotropic probes, not 16.
    pub const TERRAIN_ANISO_8: u32 = 2;
    /// The same with at most 4.
    pub const TERRAIN_ANISO_4: u32 = 4;
    /// The ground textures' larger copies (the 4x distance blend, rock's 8x and 64x scales) with
    /// at most 4 anisotropic probes; the texture itself keeps 16.
    pub const TERRAIN_COPY_ANISO: u32 = 8;
    /// A ground texture's probes follow how much of it still shows: 16 while it is all there,
    /// down to 2 as it fades into its mean color with distance.
    pub const TERRAIN_FADE_ANISO: u32 = 16;
    /// The larger copies with one lookup each instead of the anti-tiling pair.
    pub const TERRAIN_COPY_SINGLE: u32 = 32;
    /// Where every tap of the cascades' blocker search finds a caster the point is taken as
    /// fully shadowed and the filter taps (up to `shadow_filter_taps`) are not read.
    pub const SHADOW_UMBRA: u32 = 64;
    /// The screen-space sun marches with fewer steps: contact shadows 6 instead of 10, the
    /// march past the cascades 5 instead of 8.
    pub const SUN_MARCH_STEPS: u32 = 128;
    /// The sky a reflection sees marched with 9 taps instead of 18 over the same reach.
    pub const SPECULAR_SKY_TAPS: u32 = 256;
    /// The sun's shadows, the sky's cover and the probes' light worked out once per pixel in a
    /// screen pass after the prepass (screen_light.wgsl) and loaded by shading, instead of in
    /// every run of every pixel's shader: a pixel's shader runs once for each triangle that
    /// touches its 2x2 block, two to three times a pixel where triangles are a few pixels
    /// across.
    pub const SCREEN_LIGHT: u32 = 512;
    /// Lamps sorted into 16-pixel tiles after the depth pass (lamp_tiles.wgsl): a pixel's
    /// shader goes through the lamps that can reach its tile instead of every lamp in view
    /// (61 at the plaza, 153 at the overview). Same picture: a lamp left out reaches no
    /// surface in the tile.
    pub const TILED_LAMPS: u32 = 1024;
    /// The fog and haze in front of every pixel the prepass drew, worked out once a pixel in
    /// the screen light pass (the froxels, the height fog beyond them, the aerial table) and
    /// loaded by `surface_fog`, instead of in every run of every pixel's shader.
    pub const SCREEN_FOG: u32 = 2048;
    /// Ground seen edge on is drawn without every second or fourth row of its triangles across
    /// the view (`engine_terrain`: `TerrainSettings::skip_lines_px`): there they are a few
    /// pixels wide and one or two tall, and every pixel's shader runs once for each that
    /// touches its 2x2 block.
    pub const GROUND_LINES: u32 = 4096;
    /// The same, and the triangles left may look half again as long across the view as they
    /// are wide (with `GROUND_LINES` alone, no longer than wide).
    pub const GROUND_LINES_WIDE: u32 = 8192;
    /// The ground's color worked out once a pixel instead of once for every triangle that
    /// touches the pixel's 2x2 block (terrain.wgsl "the ground once a pixel"): the patches are
    /// drawn once more to write down what their vertices carry at each pixel (52 bytes a
    /// pixel while it is on), a triangle over the screen colors the pixels from that, and the
    /// scene pass's patches fail their depth test there before any shader runs.
    pub const GROUND_ONCE: u32 = 16384;
    /// Meshes work out their material once: today the depth prepass works it out for the
    /// normals and roughness, at every layer it draws, and the scene pass works it out again
    /// at every pixel before lighting it. With this on (main view, mesh_feature.rs "once"),
    /// the opaque meshes leave the shared prepass for a pass of their own right after it,
    /// which also stores what lighting needs (36 bytes a pixel while it is on); the scene
    /// pass draws none of them and a triangle over the screen lights each pixel once, not
    /// once for every triangle that touches its 2x2 block. Impostors and virtual geometry
    /// keep the old way.
    pub const MESHES_ONCE: u32 = 32768;
    /// The opaque meshes' scene program runs after the depth test, not before it. Their
    /// programs can write a virtual texture request, and a program that writes anywhere but
    /// its targets must run before the depth test unless it asks for the test first: without
    /// this the scene pass shades every layer of meshes and throws away all but the one
    /// whose depth equals the prepass's. With it (a device with `SHADER_EARLY_DEPTH_TEST`:
    /// Vulkan), `fs_main_early` asks. Same picture.
    pub const MESHES_EARLY_Z: u32 = 65536;
    /// With [`MESHES_ONCE`]: the meshes' own pass draws their depth first (a program that
    /// only discards what the material would) and then their material where the depth is
    /// equal, after the test, so the material is worked out for the layer in view only and
    /// not for every layer drawn. Their vertices are drawn twice, as today. Alpha-tested
    /// materials (leaves) are left out of the depth step and drawn whole, as with
    /// `MESHES_ONCE` alone: their depth takes their material.
    pub const MESHES_DEPTH_FIRST: u32 = 131072;
    /// With [`MESHES_DEPTH_FIRST`]: alpha-tested materials go through the depth step too,
    /// which reads only their base color to find their holes (`evaluate_material_with` in
    /// mesh.wgsl), so their whole material (normal map, surface maps, detail, weather, snow)
    /// is worked out for the layer in view only as well. Costs their vertices drawn twice
    /// and one texture read for each layer.
    pub const MESHES_HOLES_FIRST: u32 = 262144;
    /// Opaque meshes are drawn in bands of distance, the nearest band first (within a
    /// pipeline; `Item::sort_key` in mesh_feature.rs), so that what stands behind is drawn
    /// later, fails the depth test and its material is not worked out: fewer layers pass
    /// the test in the prepass (or in the meshes' own pass), at no cost on the GPU. Costs
    /// the CPU more batches (a mesh in view in several bands is a batch in each) and binds.
    /// Same picture.
    pub const MESHES_NEAR_FIRST: u32 = 524288;
}

/// What the screen shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum DebugView {
    #[default]
    None,
    /// World-space normals.
    Normals,
    /// Shadow cascades tinted.
    Cascades,
    /// How much open sky each surface sees (white = outdoors, black = cave or interior).
    SkyVisibility,
    /// Screen-space ambient occlusion (white = open, black = crevices and contact).
    AmbientOcclusion,
    /// Screen-space reflections: what each surface reflects from the screen, times how much of
    /// its reflection that covers (black where only the sky is reflected).
    Reflections,
    /// Screen-space global illumination: the light bounced onto each surface from nearby
    /// surfaces (before it is multiplied by the surface's color).
    IndirectLight,
    /// Virtual geometry: every cluster in its own color (other meshes gray).
    Clusters,
    /// Virtual geometry: each cluster's level of detail, from blue (full detail) through cyan,
    /// green, yellow, orange, red and magenta to white (7 or more halvings).
    ClusterLevels,
    /// Virtual textures: the level each surface's first virtual texture shows, in the colors of
    /// `ClusterLevels`, with tile edges darkened; reddish where a coarser tile stands in for one
    /// still loading. Surfaces without virtual textures are gray.
    VirtualTextures,
}

/// Renderer-wide quality and look settings.
#[derive(Resource, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RenderSettings {
    /// Present with vsync.
    pub vsync: bool,
    /// Internal resolution relative to the camera's viewport (the window without a
    /// [`Viewport`](crate::Viewport)), upscaled to it (`temporal_upscaling`): 0.5 renders a quarter of
    /// the pixels. 0.25..2; the game's [`Scalability::render_scale`] multiplies it.
    pub render_scale: f32,
    /// Under a `render_scale` below 1 with temporal anti-aliasing, rebuild the viewport's full
    /// resolution from the jittered frames (temporal upscaling, as Unreal's TAAU and AMD's
    /// FSR 2) instead of stretching the smaller image.
    pub temporal_upscaling: bool,
    /// Shadows are drawn up to this distance from the camera, meters.
    pub shadow_distance: f32,
    /// Shadow map resolution per cascade and of the far shadow map (applied at startup).
    pub shadow_map_size: u32,
    /// Bits of depth in the sun's shadow maps, their static images and people's insets: 16
    /// (the default) or 32 (float; applied at startup, any other value is 32). The two cast the
    /// same shadows (tests/shadow_depth.rs). A shadow map's depth is linear
    /// over its own slab, so 16 bits are 65536 steps of it, a tenth to a third of a texel's
    /// width for the cascades, under the receiver's offset along its normal. At Ultra the maps
    /// and static images are ten layers of 4096 x 4096: 640 MB at 32 bits, 320 MB at 16.
    /// `ENGINE_SHADOW_DEPTH=16|32` overrides it for side-by-side runs.
    pub shadow_depth_bits: u32,
    /// Far shadows: beyond `shadow_distance`, big casters (terrain, mountains, cliffs, large
    /// buildings) still cast sun shadows out to this distance from the camera, meters, from one
    /// coarse shadow map (`shadow_map_size`², so 8 km gives texels of about 8 m at 2048). Mountains
    /// shade the valley at low sun and ridges shade themselves. 0 = off (no sun shadows beyond
    /// `shadow_distance`).
    pub far_shadow_distance: f32,
    /// 0 = uniform cascade splits, 1 = logarithmic.
    pub shadow_split_lambda: f32,
    /// Shadow maps keep a static image of what holds still (`shadows::ShadowCache`), drawn
    /// again only when a map moves past its margin, the sun turns or what is in it changes;
    /// each frame it is copied and the moving casters are drawn on top, so every map is current
    /// every frame. 0 draws every caster into every map every frame (for comparisons); any
    /// other value keeps the static images.
    pub shadow_cascade_reuse: u32,
    /// A far shadow map whose still casters changed (one started to move, was edited or
    /// removed) is drawn again at most this often, seconds: the third cascade at this interval,
    /// the last cascade and the sky occlusion map at twice it, the far shadow map at four times
    /// (`shadows::static_holds`). The two nearest cascades never wait. In a busy scene something
    /// changes somewhere in a far map on most frames, and each change draws every still caster
    /// in it again. 0 = at once (the default).
    pub shadow_refresh_secs: f32,
    /// Record and submit on a render thread (see [`Pipelining`]).
    pub pipelining: Pipelining,
    /// Split each frame's commands into a few encoders and finish (encode) all but the last on
    /// worker threads while the rest is recorded (`finish::Finisher`); off: one encoder,
    /// finished on the render thread.
    pub parallel_encoding: bool,
    pub shadow_depth_bias: f32,
    /// Normal offset in shadow texels.
    pub shadow_normal_bias: f32,
    /// Smallest shadow filter radius in texels (the edge is never sharper than this).
    pub shadow_softness: f32,
    /// Scale of the sun's physical penumbra (1 = the real sun's size: shadows are crisp at
    /// contact and soften with distance from the caster; 2 = twice as soft).
    pub sun_penumbra_scale: f32,
    /// Screen-space contact shadows: each lit pixel marches this far (meters) toward the sun
    /// through the depth buffer, catching small shadows the shadow map is too coarse for
    /// (pebbles, grass, feet on the ground). 0 = off.
    pub contact_shadow_length: f32,
    /// Shadow-map filter taps per pixel for the sun's soft shadows (PCSS filter step, 1..64).
    /// Planar reflections take half (`for_reflection`).
    pub shadow_filter_taps: u32,
    /// Planar reflections pick levels of detail as if things were this many times farther
    /// (Unreal's planar reflection "LOD Distance Factor"): coarser meshes and clusters, and
    /// small things culled sooner, in a picture seen through ripples. 1 = as the main view.
    pub reflection_lod_distance_factor: f32,
    /// Ground-truth ambient occlusion (GTAO) from the depth prepass: darkens ambient light in
    /// creases, under objects and where things touch.
    pub ambient_occlusion: bool,
    /// How far (meters) occluders are searched for around each pixel.
    pub ao_radius: f32,
    /// 0..2 strength of the ambient occlusion (1 = physical).
    pub ao_intensity: f32,
    /// Screen-space reflections: smooth and glossy surfaces reflect what is on screen (as of
    /// the previous frame), blending into the prefiltered sky where the reflection leaves the
    /// screen or is hidden.
    pub screen_space_reflections: bool,
    /// Surfaces rougher than this (perceptual roughness) reflect only the sky.
    pub ssr_max_roughness: f32,
    /// World reflections: while probe GI is on, what screen-space reflections can't see (off
    /// screen, or hidden behind something) is traced against the probes' scene at half
    /// resolution and lit like a probe ray's hit (sun and lamps through shadow rays, glow,
    /// bounce light from the probes), instead of reflecting the sky. Needs
    /// `screen_space_reflections` and `probe_gi`.
    pub world_reflections: bool,
    /// Screen-space global illumination: diffuse light bounced off nearby surfaces that are on
    /// screen (sunlit walls tint the ground, lamps and emissive surfaces light their
    /// surroundings). Builds up over a few frames.
    pub screen_space_gi: bool,
    /// Strength of the bounced light (1 = physical).
    pub gi_intensity: f32,
    /// Probe GI (DDGI): a grid of light probes around the camera traces rays against the
    /// meshes around it, so rooms, caves and streets at night get light bounced off everything
    /// near, on screen or not: a sunlit patch on the floor lights the ceiling, a torch the walls
    /// around it, and closed rooms keep the sky out without a `SkyLightVolume`. Where the grid
    /// covers, it replaces the sky visibility guess for diffuse ambient light. On from High
    /// quality (`ENGINE_PROBE_GI=on|off` overrides it in any game); `render.gi.*` diagnostics.
    pub probe_gi: bool,
    /// Meters between probes (smaller: finer light, a smaller grid).
    pub gi_probe_spacing: f32,
    /// Probes along x, y and z (at most 16 384 in all).
    pub gi_probe_grid: [u32; 3],
    /// Probes traced per frame at most (probes the camera's move brings in first, then the rest
    /// in turn).
    pub gi_probes_per_frame: u32,
    /// GPU milliseconds a frame the probes may take: timestamps measure their cost per ray and
    /// each frame traces the probes that fit, up to `gi_probes_per_frame`. 0, a software GPU or
    /// a GPU without timestamps: always `gi_probes_per_frame`.
    pub gi_budget_ms: f32,
    /// Rays per traced probe, 16..256.
    pub gi_rays_per_probe: u32,
    /// How much of a probe's light carries over to its next update, 0..0.995 (higher: smoother,
    /// slower to follow changing light).
    pub gi_hysteresis: f32,
    /// Point lights (the nearest first) the probes' rays see, through shadow rays.
    pub gi_shadowed_lights: u32,
    /// How far light spreads under skin, meters at full strength (screen-space subsurface
    /// scattering of the pixels a feature marks; see `RenderFeature::needs_subsurface`).
    /// 0 = off.
    pub subsurface_width: f32,
    /// Scales the number of blades in every `GrassField` (quality setting; 0 = no grass).
    pub grass_density: f32,
    /// Volumetric fog: the height fog ([`FogSettings`]) and every
    /// [`FogVolume`](crate::components::FogVolume) are lit per froxel (a cell of a grid
    /// fitted to the view) by the sun with its shadows, the sky and point lights, so sunbeams
    /// show through trees and windows, lamps glow in mist and caves stay dark. Off: the height
    /// fog is an unshadowed analytic integral and fog volumes are not drawn.
    pub volumetric_fog: bool,
    /// How far from the camera the volumetric fog grid reaches, meters. Beyond it the height
    /// fog continues analytically (without shadows or fog volumes).
    pub volumetric_fog_range: f32,
    /// Size of the volumetric fog grid (tile size and depth slices).
    pub volumetric_fog_quality: VolumetricQuality,
    pub anti_aliasing: AntiAliasing,
    /// 0..1 sharpening after temporal anti-aliasing (restores texture detail).
    pub sharpness: f32,
    /// 0..1: how much temporal anti-aliasing keeps thin things the transparent phase draws
    /// (falling snowflakes, rain streaks, sparks), which write no depth or motion and would
    /// otherwise fade to a tenth of their strength. Found automatically; wide transparent
    /// surfaces (water, glass, smoke) stay anti-aliased. 0 = off.
    pub taa_reactive: f32,
    pub tonemapper: Tonemapper,
    /// 0..1 mix of the blurred image (0.04 is subtle and physical).
    pub bloom_intensity: f32,
    /// The most a pixel brings into the bloom, in multiples of white at the current exposure
    /// (pre-exposed). Above half of it brightness rolls off smoothly toward it, so lights far
    /// brighter than the exposure (a neon ring at night, the sun) glow without swamping the
    /// frame, while everything up to half of it blooms as before. 0 = no limit.
    pub bloom_limit: f32,
    /// The sun's glare: light scattered around the sun by the eye or a lens, so it reads as
    /// the sun and not a white dot (`sun_glare` in post.wgsl). Fades as trees, clouds or mist
    /// cover the disc. 1 is the eye's full glare, 0 none.
    pub sun_glare: f32,
    pub saturation: f32,
    pub contrast: f32,
    pub vignette: f32,
    pub film_grain: f32,
    /// Warm (+) / cool (−) tint.
    pub white_balance: f32,
    /// Maximum point lights shaded per frame (nearest first).
    pub max_point_lights: u32,
    pub debug_view: DebugView,
    /// Parts of the shading switched off to measure what they cost on the GPU (bits of
    /// [`shade_skip`]; 0 = none, the default). The picture is wrong while any is set: for
    /// `tools/engine-mcp`'s `gpu_sweep` and A/B runs only.
    pub debug_skip: u32,
    /// Cheaper ways to draw the same picture that are being measured before they become the
    /// way it is drawn (bits of [`trial_cut`]; 0 = none, the default). For `gpu_sweep` and
    /// side-by-side pictures: a cut that saves time and shows no difference loses its bit and
    /// becomes the code, one that doesn't is removed.
    pub trial_cuts: u32,
    /// The frame rate the work spread over frames is set for: probe tracing's time a frame
    /// (`gi_budget_ms`), the environment map's interval (`environment_interval`) and, in the
    /// weather module, the share of the cloud image marched a frame. At a higher rate each frame
    /// does less of it, so a second holds the same work whatever the rate ([`FramePacing`]):
    /// at twice this rate, half the tracing time and the map every other interval. A frame at
    /// 120 a second is a quarter as long as one at 30, and the light it spreads over frames
    /// changes no faster. 0 = the same work every frame, whatever the rate (the default).
    pub pacing_fps: f32,
    /// Write a `perf:` line to the log every this many seconds: frames per second, the render
    /// system's CPU time and its wait for the window image, everything else's CPU time, the
    /// GPU's time and memory (see `perf.rs`). 0 = never.
    pub perf_log_secs: f32,
    /// Re-render the sky environment map (the sky light and its reflections) every N frames
    /// (1 = every frame), and at once after a camera cut, when the sun turns 0.2° or the camera
    /// travels 50 m. The sky changes slowly, so 4 looks the same as 1 for a quarter of the cost
    /// (Unreal's real-time sky light capture is time-sliced the same way).
    pub environment_interval: u32,
    /// A mirror's view ([`crate::PlanarReflection`]) lights the reflected scene with the main
    /// view's last sun shadow maps, sky occlusion, environment, cloud shadow and surface
    /// weather maps instead of drawing its own: all of them are centered on the camera's
    /// position, which a horizontal mirror's camera shares (Unreal's planar reflections and
    /// HDRP's planar probe likewise skip shadow maps of their own). Off: the mirror draws its
    /// own every frame. `ENGINE_MIRROR_SHARES_LIGHTING=on|off` overrides it.
    pub mirror_shares_lighting: bool,
    /// The haze and the atmosphere between the camera and distant surfaces come from a small
    /// table built once a frame (`aerial.rs`, after Hillaire 2020's aerial perspective volume)
    /// instead of being marched for every pixel: one lookup where each pixel took an 8-step
    /// atmosphere march and 8 haze shadow samples. Off: every pixel marches.
    /// `ENGINE_AERIAL_TABLE=on|off` overrides it.
    pub aerial_table: bool,
    /// Occlusion culling of the camera's instanced meshes on the GPU (`mesh_cull.rs`, two
    /// phases against the depth: what hills, walls and trees hide isn't drawn, the picture is
    /// the same). Needs `multi_draw_indirect`. `ENGINE_MESH_OCCLUSION=on|off` overrides it.
    pub mesh_occlusion_culling: bool,
    /// The sky's own flat cloud layer (a 2D deck 1.5 km up whose cover follows
    /// `WeatherState::cloud_cover`), in the sky and in the environment map that lights the
    /// scene. Turn it off when a feature draws its own clouds over the sky (volumetric clouds):
    /// the sky behind them is then clear.
    pub sky_cloud_layer: bool,
    /// Top-down sky occlusion map around the camera (caves and interiors lose sky light; rain
    /// stays off covered ground).
    pub sky_occlusion: bool,
    /// Width of the area it covers, meters (resolution is `sky_occlusion_size`²).
    pub sky_occlusion_extent: f32,
    pub sky_occlusion_size: u32,
    /// Light through leaf canopies (`crate::canopy`): swaying leaf crowns go into a top-down map
    /// of leaf area out to 512 m instead of the sky occlusion map, and the sky light under and
    /// inside them is what passes through their leaves (three rings of sky, Goudriaan's crop
    /// canopy model) rather than all or nothing. Needs `sky_occlusion`. `ENGINE_CANOPY=on|off`
    /// overrides it in any game.
    pub canopy: bool,
    /// Leaf area seen by the canopy, times this (1 = the leaf area of the crowns' cards).
    pub canopy_strength: f32,
    /// Sun shadows past the cascades' reach from the depth buffer (`distant_shadow` in
    /// lib_frame.wgsl): houses, trees and rocks there aren't in the far shadow map (big casters
    /// only), so without this they lose their shadows past `shadow_distance`. Scales how far
    /// each pixel marches toward the sun (1 = 6 % of its distance, 12-60 m); 0 = off.
    /// `ENGINE_DISTANT_SHADOWS=on|off`.
    pub distant_shadows: f32,
    /// Inset shadows (characters week R1): the people nearest the camera, within
    /// [`crate::shadows::INSET_DISTANCE_M`], each get a sun shadow map of their own around their
    /// body (`shadow_inset_size`² texels, a millimeter or two each), so a nose, a hat brim or an
    /// arm shades the face and body as sharply as Unreal's per-object shadows do. Drawn by the
    /// features that draw people (`RenderFeature::draw_inset_shadow`). At most
    /// [`crate::shadows::MAX_SHADOW_INSETS`]; 0 = off.
    pub shadow_insets: u32,
    /// Texels across each inset shadow map.
    pub shadow_inset_size: u32,
    /// Cloud shadows: when a module draws the cloud shadow map (the weather's clouds; see
    /// `cloud_shadows`), the sunlight on every surface and in the fog is shaded by it. Off: the
    /// map is not drawn and nothing is shaded by clouds. `ENGINE_CLOUD_SHADOWS=on|off` overrides
    /// it (two runs' clouds drift apart, so before/after pictures of the ground compare without).
    pub cloud_shadows: bool,
    /// How many EV darker than "average gray" very dark scenes are shown (0 = a moonlit night
    /// is exposed like noon; 2.5 = night looks like night). Ramps in below EV100 ≈ 5.
    pub night_exposure_bias: f32,
    /// How many EV darker than "average gray" caves and interiors are shown by day, as games
    /// grade them, so they read darker than the valley even after the eye adapts. Fades in by
    /// how far the meter reads below gray things in the daylight here: from 2 EV below (deep
    /// shade) to fully at 4.5 EV below (a cave, a dim room lit through its windows). Does not
    /// add to `night_exposure_bias`; the larger of the two applies. 0 = off.
    pub interior_exposure_bias: f32,
    /// Up to how many EV brighter than "average gray" auto exposure shows a scene that is
    /// brighter than gray things would be in the light falling on it: a sunlit snowfield, a sea
    /// of cloud tops, white sand. The meter alone would show them gray, as a camera's does; this
    /// opens up as a photographer does over snow, by how far the meter reads above the light
    /// (less while facing the sun, whose glow brightens the sky, not the scene). 0 = off.
    pub bright_scene_exposure: f32,
    /// 0..1 loss of color in very dark areas (scotopic vision: moonlight looks blue-gray).
    pub night_desaturation: f32,
    /// Virtual geometry: dense opaque meshes are drawn as clusters whose level of detail and
    /// culling are picked on the GPU every frame (see `virtual_geometry`). Off = every mesh
    /// takes the regular instanced path.
    pub virtual_geometry: bool,
    /// Meshes with at least this many triangles take the cluster path. Smaller ones (props,
    /// debris, fragments) are cheaper instanced; the `NoVirtualGeometry` marker keeps a dense
    /// mesh off it too. Meshes with a `Displacement` take it whatever their triangle count.
    pub virtual_geometry_min_triangles: u32,
    /// Occlusion culling of virtual geometry: clusters hidden behind what the depth prepass
    /// drew (tested against last frame's depth, then this frame's) are not drawn.
    pub virtual_geometry_occlusion: bool,
    /// Keep built cluster hierarchies (and displaced tessellations) on disk, keyed by a hash of
    /// the mesh and its displacement, so the next start loads them instead of building again.
    pub virtual_geometry_cache: bool,
    /// Folder of that cache; empty: `ENGINE_VG_CACHE`, else the user cache folder's
    /// `engine/virtual-geometry`.
    pub virtual_geometry_cache_dir: String,
    /// Size budget of the cache folder, megabytes (least recently used builds go first).
    pub virtual_geometry_cache_mb: u32,
    /// Largest geometric error a cluster may show on screen, pixels (lower = more triangles).
    pub cluster_error_px: f32,
    /// Levels of detail of instanced meshes (those not on the cluster path): each view draws
    /// the coarsest level whose surface strays at most this many pixels (or shadow-map texels)
    /// from the full mesh. Built on a worker thread for meshes of 512 triangles or more.
    /// 0 = always the full mesh.
    pub mesh_lod_pixels: f32,
    /// Instanced meshes narrower than this many pixels on screen (or texels in a shadow map)
    /// are left out of that view. 0 = draw everything.
    pub min_object_pixels: f32,
    /// Shadow casters narrower than this many texels of a sun cascade skip that cascade (at
    /// least `min_object_pixels`): their shadow is under the filter's width, so a far cascade
    /// doesn't draw every pebble, plant part and prop in its 100 m squares. Unreal's
    /// `r.Shadow.RadiusThreshold` culls small casters the same way, by screen radius. Scaled by
    /// `Scalability::shadows` (smaller at higher quality).
    pub shadow_caster_min_texels: f32,
    /// Instanced meshes whose levels of detail stay above a few hundred triangles (leaf-card
    /// trees, ragged props) are drawn as impostors, one baked quad each, once smaller than this
    /// many pixels on screen (or texels in a shadow map); see `impostor.rs`. On screen they
    /// cross-fade, from this size down to 70% of it. 0 = never.
    pub impostor_pixels: f32,
    /// Most meshes baked into the impostor atlas at once. A full atlas reuses the layer of the
    /// mesh drawn as an impostor longest ago (once unseen for 300 frames). Layers are BC7 (1.5 MB
    /// each) where textures are compressed (`texture_compression`); otherwise RGBA8 (6 MB each)
    /// and at most 32.
    pub impostor_atlas_layers: u32,
    /// The same for shadow maps, in shadow map texels (cascade texels are much larger than
    /// screen pixels, so shadows use coarser clusters than the camera). The default of 2 halves
    /// the shadow triangles of 1 with no visible change under the shadow filter.
    pub cluster_shadow_error_texels: f32,
    /// Texels per side of each map of a generated material preset (`StandardMaterial::preset`):
    /// a power of two, 64..4096. Each preset is generated once per size, on first use.
    pub material_texture_size: u32,
    /// Free an image's CPU pixels (`Image::data` and `mips`) once the renderer has uploaded it
    /// and it has stayed unchanged for 120 frames, keeping only the GPU copy (the size, format
    /// and flags stay; see `image_cache`). Saves the CPU copy of every texture (gigabytes in
    /// big worlds; under a software GPU the GPU copy is system memory too). Images that keep
    /// changing are never released, and assigning new `data` uploads again. Images other code
    /// reads on the CPU or uploads itself need `Image::keep_pixels`. On by default.
    pub release_image_pixels: bool,
    /// Streamed virtual texturing (see `virtual_texture`): big images and the textures of
    /// `VirtualTextures` are drawn through a fixed-size tile cache, streaming only the tiles the
    /// screen needs. Off = big images are uploaded whole (as far as the GPU allows) and
    /// `VirtualTextures` handles show their preview.
    pub virtual_textures: bool,
    /// RGBA8 images wider or taller than this many texels are tiled into the virtual texture
    /// cache instead of being uploaded whole (when their size halves evenly down to one tile).
    pub virtual_texture_threshold: u32,
    /// Tiles (128² texels plus an 8-texel border) per cache texture; there is one for sRGB
    /// color and one for linear data, each created on first use. 1024 is about 113 MB each as
    /// RGBA8, 28 MB with BC7. Applied when changed (the cache starts over).
    pub virtual_texture_cache_tiles: u32,
    /// Most tiles uploaded to the cache per frame (texture roots don't count).
    pub virtual_texture_upload_budget: u32,
    /// Megabytes of new meshes and images uploaded per frame in play (0: no limit); the rest
    /// waits for the next frames (`render.pending_upload_mb`). Behind a loading screen
    /// (`LoadingBudget`) uploads run for its `budget_ms` instead. See `UploadBudget`. Headless
    /// apps (tests, tools, tours) upload without a limit unless `upload_budget_headless` is set,
    /// so each frame they draw has everything.
    pub upload_mb_per_frame: f32,
    /// Apply `upload_mb_per_frame` headless too (for tests of the budget).
    pub upload_budget_headless: bool,
    /// Draw runs of mesh batches that share a pipeline, texture set and vertex pool page with
    /// one `multi_draw_indexed_indirect` each (where the GPU supports a first instance in
    /// indirect draws), instead of a draw call per mesh and level of detail.
    pub multi_draw_indirect: bool,
    /// Compile every mesh pipeline variant on worker threads once the shaders exist (after
    /// Unreal's PSO precaching), so neither the first frame nor the first sight of a new kind
    /// of material waits for a shader compile.
    pub pipeline_warmup: PipelineWarmup,
    /// Added to the level of detail virtual textures ask for (positive = coarser, fewer tiles).
    pub virtual_texture_mip_bias: f32,
    /// BC7-compress the cache where the GPU supports it (a quarter of the memory; encoded on the
    /// tile loader threads). Applied when changed.
    pub virtual_texture_compression: bool,
    /// Block-compress material images on the GPU once they have been drawn unchanged for a
    /// moment (`image_cache`): a quarter of the memory and texture bandwidth of RGBA8 (an
    /// eighth for BC1 and BC4). Needs a GPU with BC formats; images whose levels don't halve to
    /// multiples of 4 texels, and slots that ask for exact pixels, stay RGBA8.
    pub texture_compression: TextureCompression,
    /// Also compress under a software GPU (lavapipe), where building the encoders takes seconds
    /// and every texture sample decodes a block in software; saves system memory there. Off by
    /// default.
    pub texture_compression_on_cpu: bool,
    /// GPU time texture compression may take a frame in play (ms). Images are compressed a slice
    /// of rows at a time; the slices a frame takes follow the measured time per texel.
    pub texture_compression_ms: f32,
    /// The same while a `LoadingBudget` is present (arriving somewhere, a loading screen): the
    /// picture still updates then, so this stays a few milliseconds.
    pub texture_compression_loading_ms: f32,
}

impl Default for RenderSettings {
    fn default() -> Self {
        Self {
            vsync: true,
            render_scale: 1.0,
            temporal_upscaling: true,
            shadow_distance: 350.0,
            shadow_map_size: 2048,
            shadow_depth_bits: 16,
            far_shadow_distance: 8000.0,
            shadow_split_lambda: 0.8,
            shadow_cascade_reuse: 3,
            shadow_refresh_secs: 0.0,
            pipelining: Pipelining::Windowed,
            parallel_encoding: true,
            shadow_depth_bias: 0.0,
            shadow_normal_bias: 1.5,
            shadow_softness: 1.0,
            sun_penumbra_scale: 1.0,
            contact_shadow_length: 0.4,
            shadow_filter_taps: 16,
            reflection_lod_distance_factor: 2.0,
            ambient_occlusion: true,
            ao_radius: 1.5,
            ao_intensity: 1.0,
            screen_space_reflections: true,
            ssr_max_roughness: 0.6,
            world_reflections: true,
            screen_space_gi: true,
            gi_intensity: 1.0,
            probe_gi: true,
            gi_probe_spacing: 1.5,
            gi_probe_grid: [32, 16, 32],
            gi_probes_per_frame: 2048,
            gi_budget_ms: 1.5,
            gi_rays_per_probe: 64,
            gi_hysteresis: 0.9,
            gi_shadowed_lights: 8,
            subsurface_width: 0.012,
            grass_density: 1.0,
            volumetric_fog: true,
            volumetric_fog_range: 256.0,
            volumetric_fog_quality: VolumetricQuality::High,
            anti_aliasing: AntiAliasing::Taa,
            sharpness: 0.3,
            taa_reactive: 1.0,
            tonemapper: Tonemapper::AgXPunchy,
            bloom_intensity: 0.04,
            bloom_limit: 64.0,
            sun_glare: 0.04,
            saturation: 1.05,
            contrast: 1.0,
            vignette: 0.2,
            film_grain: 0.004,
            white_balance: 0.0,
            max_point_lights: 256,
            debug_view: DebugView::None,
            debug_skip: 0,
            trial_cuts: 0,
            pacing_fps: 0.0,
            perf_log_secs: 10.0,
            environment_interval: 4,
            mirror_shares_lighting: true,
            aerial_table: true,
            mesh_occlusion_culling: true,
            sky_cloud_layer: true,
            sky_occlusion: true,
            sky_occlusion_extent: 256.0,
            sky_occlusion_size: 512,
            canopy: true,
            canopy_strength: 1.0,
            distant_shadows: 1.0,
            shadow_insets: crate::shadows::MAX_SHADOW_INSETS as u32,
            shadow_inset_size: 2048,
            cloud_shadows: true,
            night_exposure_bias: 2.5,
            interior_exposure_bias: 1.25,
            bright_scene_exposure: 2.0,
            night_desaturation: 0.7,
            virtual_geometry: true,
            virtual_geometry_min_triangles: 50_000,
            virtual_geometry_occlusion: true,
            virtual_geometry_cache: true,
            virtual_geometry_cache_dir: String::new(),
            virtual_geometry_cache_mb: 2048,
            cluster_error_px: 1.0,
            mesh_lod_pixels: 1.0,
            min_object_pixels: 1.0,
            shadow_caster_min_texels: 3.0,
            impostor_pixels: 96.0,
            impostor_atlas_layers: 128,
            cluster_shadow_error_texels: 2.0,
            material_texture_size: 1024,
            release_image_pixels: true,
            virtual_textures: true,
            virtual_texture_threshold: 4096,
            virtual_texture_cache_tiles: 1024,
            virtual_texture_upload_budget: 32,
            upload_mb_per_frame: 16.0,
            upload_budget_headless: false,
            multi_draw_indirect: true,
            pipeline_warmup: PipelineWarmup::Windowed,
            virtual_texture_mip_bias: 0.0,
            virtual_texture_compression: true,
            texture_compression: TextureCompression::Quality,
            texture_compression_on_cpu: false,
            texture_compression_ms: 1.0,
            texture_compression_loading_ms: 4.0,
        }
    }
}

/// How much of the work spread over frames this frame does ([`RenderSettings::pacing_fps`]):
/// the renderer keeps it from the frame rate of the last half second or so. Features that
/// spread work of their own read `share` (the weather's cloud march does).
#[derive(Resource, Debug, Clone, Copy, PartialEq)]
pub struct FramePacing {
    /// 1 at the pacing rate and below it (and with pacing off), falling to 0.25 at four times
    /// the rate: the pacing rate over the frame rate.
    pub share: f32,
    /// The smoothed frame time, seconds (0 before the first frame).
    frame_s: f32,
}

impl Default for FramePacing {
    fn default() -> Self {
        Self {
            share: 1.0,
            frame_s: 0.0,
        }
    }
}

impl FramePacing {
    /// Seconds over which the frame time is smoothed: one slow frame doesn't move the share.
    const SMOOTH_S: f32 = 0.5;

    /// The frame time of the last half second or so, seconds (0 before the first frame).
    pub fn frame_secs(&self) -> f32 {
        self.frame_s
    }

    /// Take a frame of `dt` seconds (wall clock) at `pacing_fps`.
    pub fn advance(&mut self, dt: f32, pacing_fps: f32) {
        if !(dt.is_finite() && dt > 0.0) {
            return;
        }
        self.frame_s = if self.frame_s > 0.0 {
            // A hitch is one long frame, not a slow rate: it counts as a frame twice as long as
            // the frames lately at most, so the rate has to stay low to bring the share up.
            let dt = dt.min(self.frame_s * 2.0);
            let k = 1.0 - (-dt / Self::SMOOTH_S).exp();
            self.frame_s + (dt - self.frame_s) * k
        } else {
            dt
        };
        self.share = if pacing_fps.is_finite() && pacing_fps > 0.0 {
            (pacing_fps * self.frame_s).clamp(0.25, 1.0)
        } else {
            1.0
        };
    }
}

/// The settings the renderer draws with: [`RenderSettings`] at the game's [`Scalability`]
/// (unscaled without one) and at the frame rate's [`FramePacing`].
pub(crate) fn effective(world: &World) -> RenderSettings {
    let settings = world.resource::<RenderSettings>();
    let mut s = match world.get_resource::<Scalability>() {
        Some(quality) => settings.scaled(quality),
        None => settings.clone(),
    };
    if let Some(pacing) = world.get_resource::<FramePacing>() {
        s.paced(pacing.share);
    }
    static PROBE_GI: std::sync::OnceLock<Option<bool>> = std::sync::OnceLock::new();
    if let Some(on) = switch_from_env("ENGINE_PROBE_GI", &PROBE_GI) {
        s.probe_gi = on;
    }
    static CANOPY: std::sync::OnceLock<Option<bool>> = std::sync::OnceLock::new();
    if let Some(on) = switch_from_env("ENGINE_CANOPY", &CANOPY) {
        s.canopy = on;
    }
    static DISTANT: std::sync::OnceLock<Option<bool>> = std::sync::OnceLock::new();
    if let Some(on) = switch_from_env("ENGINE_DISTANT_SHADOWS", &DISTANT) {
        s.distant_shadows = if on { s.distant_shadows.max(1.0) } else { 0.0 };
    }
    static CLOUD_SHADOWS: std::sync::OnceLock<Option<bool>> = std::sync::OnceLock::new();
    if let Some(on) = switch_from_env("ENGINE_CLOUD_SHADOWS", &CLOUD_SHADOWS) {
        s.cloud_shadows = on;
    }
    static MIRROR: std::sync::OnceLock<Option<bool>> = std::sync::OnceLock::new();
    if let Some(on) = switch_from_env("ENGINE_MIRROR_SHARES_LIGHTING", &MIRROR) {
        s.mirror_shares_lighting = on;
    }
    static AERIAL: std::sync::OnceLock<Option<bool>> = std::sync::OnceLock::new();
    if let Some(on) = switch_from_env("ENGINE_AERIAL_TABLE", &AERIAL) {
        s.aerial_table = on;
    }
    static OCCLUSION: std::sync::OnceLock<Option<bool>> = std::sync::OnceLock::new();
    if let Some(on) = switch_from_env("ENGINE_MESH_OCCLUSION", &OCCLUSION) {
        s.mesh_occlusion_culling = on;
    }
    // `ENGINE_TEXTURE_COMPRESSION=off|on|cpu`: material images stay RGBA8, are compressed, or
    // are compressed under a software GPU too (so a headless run on lavapipe fills and uses the
    // disk cache of compressed images as a player's GPU does), to compare GPU memory on one
    // build.
    static COMPRESSION: std::sync::OnceLock<Option<(bool, bool)>> = std::sync::OnceLock::new();
    let compression = COMPRESSION.get_or_init(|| {
        let text = std::env::var("ENGINE_TEXTURE_COMPRESSION").ok()?;
        match text.to_ascii_lowercase().as_str() {
            "off" | "0" => Some((false, false)),
            "on" | "1" => Some((true, false)),
            "cpu" => Some((true, true)),
            _ => {
                tracing::warn!("ENGINE_TEXTURE_COMPRESSION={text} is not off, on or cpu");
                None
            }
        }
    });
    match compression {
        Some((false, _)) => s.texture_compression = TextureCompression::Off,
        Some((true, on_cpu)) => {
            if s.texture_compression == TextureCompression::Off {
                s.texture_compression = TextureCompression::Quality;
            }
            s.texture_compression_on_cpu |= *on_cpu;
        }
        None => {}
    }
    // `ENGINE_SHADOW_DEPTH=16|32`: the shadow maps' depth bits, for side-by-side runs.
    static SHADOW_DEPTH: std::sync::OnceLock<Option<u32>> = std::sync::OnceLock::new();
    if let Some(bits) = SHADOW_DEPTH.get_or_init(|| {
        let text = std::env::var("ENGINE_SHADOW_DEPTH").ok()?;
        text.trim().parse().ok().filter(|b| matches!(b, 16 | 32))
    }) {
        s.shadow_depth_bits = *bits;
    }
    // `ENGINE_TEXTURE_COMPRESSION_MS=<ms>`: the GPU time compression may take a frame, in play
    // and while loading (to fill the disk cache of compressed images in one short run).
    static COMPRESSION_MS: std::sync::OnceLock<Option<f32>> = std::sync::OnceLock::new();
    let compression_ms = COMPRESSION_MS.get_or_init(|| {
        let text = std::env::var("ENGINE_TEXTURE_COMPRESSION_MS").ok()?;
        let ms = text
            .parse::<f32>()
            .ok()
            .filter(|f| f.is_finite() && *f >= 0.0);
        if ms.is_none() {
            tracing::warn!("ENGINE_TEXTURE_COMPRESSION_MS={text} is not a number of milliseconds");
        }
        ms
    });
    if let Some(ms) = compression_ms {
        s.texture_compression_ms = *ms;
        s.texture_compression_loading_ms = *ms;
    }
    // `ENGINE_GI_PROBES_PER_FRAME=<n>` caps the probes traced a frame (as a GPU budget does), to
    // see on a software GPU how the probes keep up with a moving camera.
    static GI_PROBES: std::sync::OnceLock<Option<u32>> = std::sync::OnceLock::new();
    let gi_probes = GI_PROBES.get_or_init(|| {
        let text = std::env::var("ENGINE_GI_PROBES_PER_FRAME").ok()?;
        let n = text.parse::<u32>().ok().filter(|n| *n > 0);
        if n.is_none() {
            tracing::warn!("ENGINE_GI_PROBES_PER_FRAME={text} is not a positive whole number");
        }
        n
    });
    if let Some(n) = gi_probes {
        s.gi_probes_per_frame = s.gi_probes_per_frame.min(*n);
    }
    // `ENGINE_RENDER_SCALE=<0.25..2>` multiplies the internal resolution: a pass whose GPU time
    // halves at 0.7 pays per pixel, one that doesn't per vertex or per dispatch.
    static SCALE: std::sync::OnceLock<Option<f32>> = std::sync::OnceLock::new();
    let scale = SCALE.get_or_init(|| {
        let text = std::env::var("ENGINE_RENDER_SCALE").ok()?;
        let scale = text
            .parse::<f32>()
            .ok()
            .filter(|f| f.is_finite() && *f > 0.0);
        if scale.is_none() {
            tracing::warn!("ENGINE_RENDER_SCALE={text} is not a positive number");
        }
        scale
    });
    if let Some(scale) = scale {
        s.render_scale = (s.render_scale * scale).clamp(0.25, 2.0);
    }
    s
}

/// `<name>=on|off`, when set, overrides a setting (to compare the two on one build, in any
/// game): `ENGINE_PROBE_GI` (`RenderSettings::probe_gi`), `ENGINE_MIRROR_SHARES_LIGHTING`
/// (`RenderSettings::mirror_shares_lighting`), `ENGINE_AERIAL_TABLE`
/// (`RenderSettings::aerial_table`), `ENGINE_MESH_OCCLUSION`
/// (`RenderSettings::mesh_occlusion_culling`), `ENGINE_CANOPY` (`RenderSettings::canopy`),
/// `ENGINE_DISTANT_SHADOWS` (`RenderSettings::distant_shadows`), `ENGINE_CLOUD_SHADOWS`
/// (`RenderSettings::cloud_shadows`). Read once.
fn switch_from_env(name: &str, cell: &'static std::sync::OnceLock<Option<bool>>) -> Option<bool> {
    *cell.get_or_init(
        || match std::env::var(name).ok()?.to_ascii_lowercase().as_str() {
            "off" | "0" => Some(false),
            "on" | "1" => Some(true),
            other => {
                tracing::warn!("{name}={other} is not on or off");
                None
            }
        },
    )
}

impl RenderSettings {
    /// These settings at the game's quality ([`Scalability`]): the settings are the look at
    /// `High` (every factor 1), and the factors scale the costly parts. Resolution follows
    /// `render_scale`; shadow map size and shadow distances follow `shadows`; how much detail
    /// far things keep (levels of detail, impostors, culling of tiny objects) follows
    /// `view_distance`; grass follows `density`; and `effects` turns off what costs most
    /// (volumetric fog below 0.75, screen-space reflections below 0.75, screen-space and probe
    /// GI below 1, ambient occlusion below 0.5) and thins point lights and sky updates. Nothing
    /// the game turned off is turned on.
    pub fn scaled(&self, quality: &Scalability) -> Self {
        let mut s = self.clone();
        let positive = |f: f32| if f.is_finite() { f.max(0.01) } else { 1.0 };
        s.render_scale = (self.render_scale * positive(quality.render_scale)).clamp(0.25, 2.0);

        let shadows = positive(quality.shadows);
        let size = self.shadow_map_size.max(1) as f32 * shadows;
        s.shadow_map_size = (2f32.powf(size.log2().round()) as u32).clamp(512, 8192);
        s.shadow_distance = self.shadow_distance * shadows;
        s.shadow_caster_min_texels = self.shadow_caster_min_texels / shadows;
        s.far_shadow_distance = self.far_shadow_distance * shadows;

        // Farther detail at a higher view distance: a pixel threshold shrinks as it grows.
        let reach = positive(quality.view_distance);
        s.mesh_lod_pixels = self.mesh_lod_pixels / reach;
        s.cluster_error_px = self.cluster_error_px / reach;
        s.cluster_shadow_error_texels = self.cluster_shadow_error_texels / reach;
        s.min_object_pixels = self.min_object_pixels / reach;
        s.impostor_pixels = self.impostor_pixels / reach;

        s.grass_density = self.grass_density * quality.density.max(0.0);

        let effects = quality.effects;
        s.volumetric_fog &= quality.allows(0.75);
        if effects < 1.0 {
            s.volumetric_fog_quality = self.volumetric_fog_quality.min(VolumetricQuality::Medium);
            s.environment_interval = self.environment_interval.max(1) * 2;
        }
        s.screen_space_reflections &= quality.allows(0.75);
        s.screen_space_gi &= quality.allows(1.0);
        s.probe_gi &= quality.allows(1.0);
        s.gi_budget_ms = self.gi_budget_ms * effects.max(1.0);
        s.ambient_occlusion &= quality.allows(0.5);
        if !quality.allows(0.5) {
            s.distant_shadows = 0.0;
            s.shadow_insets = 0;
        }
        let inset = self.shadow_inset_size.max(1) as f32 * shadows.min(1.0);
        s.shadow_inset_size = (2f32.powf(inset.log2().round()) as u32).clamp(256, 4096);
        s.max_point_lights =
            ((self.max_point_lights as f32 * effects.clamp(0.25, 1.0)) as u32).max(8);
        s
    }
}

impl RenderSettings {
    /// Spread the work that is spread over frames for a frame doing `share` of it
    /// ([`FramePacing::share`]): that share of the probe tracing time, the environment map
    /// every 1/share intervals.
    fn paced(&mut self, share: f32) {
        let share = share.clamp(0.25, 1.0);
        if share >= 1.0 {
            return;
        }
        self.gi_budget_ms *= share;
        self.environment_interval =
            ((self.environment_interval.max(1) as f32 / share).round() as u32).max(1);
    }
}

impl RenderSettings {
    /// These settings for a planar reflection's view ([`crate::PlanarReflection`]). The
    /// picture covers a fraction of the screen's pixels and is seen through ripples, so it keeps
    /// the look but drops what only shows up close: no screen-space contact shadows (0.4 m
    /// marches, under a pixel in the picture) and half the sun-shadow filter taps (the view's
    /// anti-aliasing averages the noise). The renderer also skips the view's bloom and
    /// tonemapping, as the picture is taken before them. HDRP does the same for reflection
    /// cameras (`FrameSettings.Sanitize` turns post-processing off), and Unreal gives planar
    /// reflections their own screen percentage and LOD distance.
    ///
    /// Levels of detail follow `reflection_lod_distance_factor`, as Unreal's "LOD Distance
    /// Factor" ("values greater than 1 ... use lower LODs than the main view to speed up the
    /// scene capture pass").
    pub fn for_reflection(&self) -> Self {
        let far = if self.reflection_lod_distance_factor.is_finite() {
            self.reflection_lod_distance_factor.clamp(0.1, 10.0)
        } else {
            1.0
        };
        Self {
            contact_shadow_length: 0.0,
            distant_shadows: 0.0,
            shadow_insets: 0,
            shadow_filter_taps: (self.shadow_filter_taps / 2).max(4),
            mesh_lod_pixels: self.mesh_lod_pixels * far,
            cluster_error_px: self.cluster_error_px * far,
            min_object_pixels: self.min_object_pixels * far,
            impostor_pixels: self.impostor_pixels * far,
            ..self.clone()
        }
    }
}

impl RenderSettings {
    /// Show colors as authored, for pure 2D art (unlit sprites and tile maps, pixel art): no
    /// tonemapping curve ([`Tonemapper::None`]: 1.0 is white), no grading, vignette, film grain,
    /// bloom or night desaturation. Turn bloom back on for glowing sprites (colors above 1).
    pub fn with_flat_colors(mut self) -> Self {
        self.tonemapper = Tonemapper::None;
        self.saturation = 1.0;
        self.contrast = 1.0;
        self.vignette = 0.0;
        self.film_grain = 0.0;
        self.white_balance = 0.0;
        self.bloom_intensity = 0.0;
        self.night_desaturation = 0.0;
        self
    }
}

/// Height fog everywhere: the global fog the weather drives. Its density falls off
/// exponentially with height above `base_height`, so it pools in valleys and thins on hills.
///
/// With `RenderSettings::volumetric_fog` (the default) it is lit per froxel, with sun shadows
/// (light shafts), sky light that caves and rooms keep out, and point lights; beyond
/// `volumetric_fog_range` and with volumetric fog off it is integrated analytically. Local fog
/// (a smoky room, a mist bank, a steam vent) is a [`FogVolume`](crate::components::FogVolume).
///
/// Weather sets `density` (it adds `WeatherState::fog * 0.02` per meter on top); artists tune
/// the rest. Ground fog at dawn: `density` 0.03–0.08, `height_falloff` 0.15–0.3.
///
/// With the [`Atmosphere`] disabled (an airless body), fog height is world y: on a planet placed
/// with `PlanetTerrain::home_transform` the globe's far side is at y ≈ −2 × its radius, and fog
/// piles up there when seen from orbit. Airless scenes should set `density` to 0.
#[derive(Resource, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FogSettings {
    /// Extinction per meter at `base_height`; the visibility there is about 3.9 / density
    /// (0.00002 clear air, 200 km; 0.0001 light haze, 40 km; 0.0005 haze, 8 km; 0.02 thick fog,
    /// 200 m; 0.1 fog you can barely see 30 m into). The atmosphere already gives distant
    /// mountains their blue haze; this adds to it, mostly along the valley floor.
    pub density: f32,
    /// How fast fog thins with height, per meter (density halves every `0.69 / falloff` m).
    pub height_falloff: f32,
    /// Altitude above the planet surface (world y when there is no planet) where `density`
    /// applies, meters.
    pub base_height: f32,
    /// Fog never hides more than this fraction of an object (keeps silhouettes). Applies to
    /// fog volumes too; 1 = physical.
    pub max_opacity: f32,
    /// Fog albedo, linear RGB: the share of the light it stops that it scatters (the rest it
    /// absorbs). Lit by the sun, the sky and lamps.
    pub color: [f32; 3],
    /// How strongly the fog scatters light forward: the Henyey-Greenstein anisotropy `g`
    /// (-0.95..0.95). 0 scatters evenly in all directions, 0.6 (default) mostly forward, so fog
    /// glows around the sun and lamps when you look toward them and sunbeams are brightest
    /// against the light.
    pub sun_scattering: f32,
}

impl Default for FogSettings {
    fn default() -> Self {
        Self {
            density: 0.0004,
            height_falloff: 0.02,
            base_height: 0.0,
            max_opacity: 1.0,
            color: [0.9, 0.93, 1.0],
            sun_scattering: 0.6,
        }
    }
}

/// The planet whose atmosphere the sky, aerial perspective and sunlight use. Space travel
/// updates this to the nearest planet (or disables it in deep space).
///
/// The air is the planet's own: what its gas (Rayleigh) and its haze or dust (Mie) do to light,
/// per meter at the surface, in the renderer's three color channels (linear sRGB), thinning
/// with height over `rayleigh_scale_height` and `mie_scale_height`.
///
/// A channel is a band of wavelengths, not one: coefficients taken at single wavelengths (the
/// usual 680, 550 and 440 nm) make a twilight sky purple, because the red channel's band reaches
/// down to the ozone's absorption peak near 600 nm and is scattered more than at 680 nm. Earth's
/// are the effective values of the whole spectrum instead: fitted so the sky rendered in these
/// three channels has the colors of the same sky rendered at 42 wavelengths from 380 to 790 nm
/// and converted with the CIE 1931 color matching functions (Bruneton's solar spectrum and ozone
/// cross-sections; within about 0.01 in chromaticity by day, at sunset and with the sun 4° and 8°
/// below the horizon).
/// The default is Earth ([`Atmosphere::earth`]); [`Atmosphere::mars`] is Mars. A planet module
/// fills these in per body; the sky's look-up tables are recomputed whenever they change.
#[derive(Resource, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Atmosphere {
    pub enabled: bool,
    /// Planet center in world coordinates, meters.
    pub planet_center: DVec3,
    /// Planet radius, meters.
    pub planet_radius: f64,
    /// Height of the atmosphere above the surface, meters.
    pub thickness: f32,
    /// Height over which the gas thins by a factor e (Rayleigh scattering), meters.
    pub rayleigh_scale_height: f32,
    /// Height over which the haze or dust thins by a factor e (Mie scattering and absorption),
    /// meters.
    pub mie_scale_height: f32,
    /// Mie anisotropy (0.76 hazy .. 0.9 clear): how strongly the haze scatters forward, into
    /// the glow around the sun. `mie_g_offset` varies it per color channel.
    pub mie_g: f32,
    /// Albedo (linear RGB) of the planet surface beyond the terrain: the ground seen from high
    /// up and toward the horizon, and the color of light bounced off the ground.
    pub ground_color: [f32; 3],
    /// Rayleigh scattering by the gas at the surface, per meter, RGB. Molecules scatter blue
    /// most (as 1/λ⁴), which makes a sky blue and a low sun red; they absorb nothing. Earth:
    /// 6.88e-6, 14.56e-6, 33.2e-6 (at 680, 550 and 440 nm alone: 5.802e-6, 13.558e-6, 33.1e-6).
    pub rayleigh_scattering: [f32; 3],
    /// Mie scattering by haze, dust or droplets at the surface, per meter, RGB: mostly forward
    /// (`mie_g`), so the sky glows around the sun and pales toward the horizon. Earth's clear
    /// air: 3.996e-6 in every channel.
    pub mie_scattering: [f32; 3],
    /// Light the haze or dust absorbs at the surface, per meter, RGB (its extinction is
    /// scattering plus absorption). Earth's haze is gray, 0.404e-6; Mars' iron-bearing dust
    /// absorbs blue most, which turns the sky it lights butterscotch.
    pub mie_absorption: [f32; 3],
    /// Absorption by an ozone layer (10–40 km up, densest at 25 km) at its densest, per meter,
    /// RGB. Ozone absorbs orange, which keeps a twilight sky blue overhead. Earth: 2.67e-6,
    /// 1.87e-6, 0.149e-6 (at 680, 550 and 440 nm alone: 0.650e-6, 1.881e-6, 0.085e-6); 0 for air
    /// without ozone.
    pub ozone_absorption: [f32; 3],
    /// Added to `mie_g` per channel (R, G, B). Dust grains a little larger than the wavelength
    /// (Mars') scatter blue into a narrower, stronger forward peak than red: a larger blue
    /// anisotropy puts a blue glow around the sun, plainest at sunset. 0 = `mie_g` for all.
    pub mie_g_offset: [f32; 3],
    /// Scales the density of the whole air, every coefficient above (1 = as given, 2 = twice as
    /// dense, 0 = no air): a season of higher pressure, a thicker variant of the same air.
    pub density_scale: f32,
    /// How much of the haze's or dust's forward peak passes as unscattered light (0..1, the
    /// delta-Eddington approximation). Grains a little larger than the wavelength (Mars' dust)
    /// scatter a good part of their light into a diffraction peak a few degrees wide, which
    /// barely turns it: the ground seen through them keeps its image and sunlight stays sunlight.
    /// One Henyey-Greenstein lobe can't hold that peak, so at 1 a share g² of the scattering
    /// (per channel) counts as passing straight on and the rest scatters with the lobe that is
    /// left, of anisotropy g / (1 + g); the glow around the sun keeps the full lobe. 0 (Earth,
    /// the default) counts all of it as scattered.
    pub mie_forward_peak: f32,
}

impl Default for Atmosphere {
    fn default() -> Self {
        Self {
            enabled: true,
            planet_center: DVec3::new(0.0, -6_360_000.0, 0.0),
            planet_radius: 6_360_000.0,
            thickness: 100_000.0,
            rayleigh_scale_height: 8_000.0,
            mie_scale_height: 1_200.0,
            mie_g: 0.8,
            ground_color: [0.12, 0.15, 0.09],
            rayleigh_scattering: [6.88e-6, 14.56e-6, 33.2e-6],
            mie_scattering: [3.996e-6; 3],
            mie_absorption: [0.404e-6; 3],
            ozone_absorption: [2.67e-6, 1.87e-6, 0.149e-6],
            mie_g_offset: [0.0; 3],
            density_scale: 1.0,
            mie_forward_peak: 0.0,
        }
    }
}

/// The air's coefficients at the surface as the shaders use them (per meter, times
/// [`Atmosphere::density_scale`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct AirOptics {
    pub rayleigh: Vec3,
    /// Mie scattering, less the forward peak that passes as unscattered
    /// ([`Atmosphere::mie_forward_peak`]).
    pub mie_scattering: Vec3,
    /// Mie scattering plus absorption, less the forward peak.
    pub mie_extinction: Vec3,
    pub ozone: Vec3,
    /// Mie anisotropy per channel (of the whole lobe, the forward peak included).
    pub mie_g: Vec3,
    /// [`Atmosphere::mie_forward_peak`]: the share g² of the scattering in the forward peak is
    /// this times g² per channel.
    pub mie_forward_peak: f32,
}

impl Atmosphere {
    /// Earth (the default): the Rayleigh, Mie and ozone of Bruneton's and Hillaire's (2020)
    /// reference atmosphere. A blue sky, red sunsets.
    pub fn earth() -> Self {
        Self::default()
    }

    /// Mars: thin CO₂ under a veil of dust, with the planet's surface at y = 0. By day the sky
    /// is butterscotch (the dust scatters red and absorbs blue); at sunset a blue glow
    /// surrounds the sun (the dust's forward peak is stronger in blue) and the sun itself stays
    /// pale instead of turning red.
    ///
    /// - Planet: mean radius 3389.5 km, the gas's scale height 11.1 km (NASA Mars fact sheet).
    /// - Gas: CO₂'s Rayleigh scattering at Mars' mean surface pressure and temperature with
    ///   the parameters of Schneegans et al. 2024, "Physically Based Real-Time Rendering of
    ///   Atmospheres using Mie Theory" (Computer Graphics Forum 43(2), Table 1: refractive
    ///   index n − 1 = 3.37e-6, depolarization 0.09), at 680, 550 and 440 nm: about 1.8 % of
    ///   Earth's.
    /// - Dust: single-scattering albedo from the same table (Mie theory for ~1.4 µm feldspar
    ///   grains with 3 % hematite, which absorbs blue): 0.96 red, 0.83 green, 0.69 blue. A
    ///   vertical optical depth of 0.34 above datum: Mars' clear season (Gale's 880 nm record,
    ///   taken 4.5 km below datum, runs from about 0.3 in the clear season to 1.5 in storms;
    ///   Spirit's fell below 0.3, Lemmon et al. 2015, "Dust aerosol, clouds, and the atmospheric
    ///   optical depth record over 5 Mars years of the Mars Exploration Rover mission", Icarus
    ///   251).
    /// - The dust thins over a 16 km scale height, not the gas's 11.1 km (which Costa et al.
    ///   2021 assumed): the same column with less of it near the ground. Lemmon et al. fit 11.9
    ///   km to Spirit's sky and 19.2 km to Opportunity's, which needed more dust up high; in the
    ///   clear season the dust's mixing ratio peaks in layers 15–25 km up (Heavens et al. 2011,
    ///   JGR 116, Mars Climate Sounder); and from inside Gale crater Curiosity measured less
    ///   extinction along its line of sight to the rim, about 30 km away, than the column
    ///   would give if well mixed, for most of the year (Moore et al. 2016, Icarus 264). Near
    ///   the ground that is 0.022 per km (red): the rim of a crater 30 km away is seen through
    ///   an optical depth of about 0.66. Below datum (canyon and crater floors) the air keeps
    ///   datum's density: those deep floors hold no more dust than the plains (Moore et al.
    ///   found Gale's floor a dust sink).
    /// - Anisotropy 0.7, rising toward blue (0.64, 0.7, 0.8): a single forward lobe can't hold
    ///   the diffraction peak, which for these grains is narrower and stronger in blue (and
    ///   stronger still where hematite absorbs blue, which Schneegans et al. name as a cause of
    ///   the blue halo around the Martian sun). Costa et al. fit a far wider spread (0.03 red,
    ///   0.4 green, 0.67 blue), which Schneegans et al. find gives a very wide blue halo;
    ///   Collienne et al. 2013, "Physically Based Rendering of the Martian Atmosphere", used a
    ///   single 0.76.
    /// - The forward peak passes as unscattered light (`mie_forward_peak` 1, delta-Eddington):
    ///   that diffraction peak is a few degrees wide for these grains, so the ground seen from
    ///   orbit keeps its detail through the haze, as it does in orbiters' images.
    /// - Ground: rusty regolith, linear albedo 0.3 red to 0.08 blue (Mars' Bond albedo is 0.25).
    pub fn mars() -> Self {
        let radius = 3_389_500.0;
        Self {
            enabled: true,
            planet_center: DVec3::new(0.0, -radius, 0.0),
            planet_radius: radius,
            thickness: 100_000.0,
            rayleigh_scale_height: 11_100.0,
            mie_scale_height: 16_000.0,
            mie_g: 0.7,
            ground_color: [0.3, 0.16, 0.08],
            rayleigh_scattering: [0.102e-6, 0.238e-6, 0.580e-6],
            mie_scattering: [21.28e-6, 17.61e-6, 14.52e-6],
            mie_absorption: [0.874e-6, 3.642e-6, 6.473e-6],
            ozone_absorption: [0.0; 3],
            mie_g_offset: [-0.06, 0.0, 0.1],
            density_scale: 1.0,
            mie_forward_peak: 1.0,
        }
    }

    /// The coefficients the shaders get: per meter at the surface, times `density_scale`.
    pub(crate) fn optics(&self) -> AirOptics {
        let k = self.density_scale.max(0.0);
        let scaled = |c: [f32; 3]| Vec3::from(c) * k;
        let g = (Vec3::splat(self.mie_g) + Vec3::from(self.mie_g_offset))
            .clamp(Vec3::splat(-0.999), Vec3::splat(0.999));
        let forward_peak = self.mie_forward_peak.clamp(0.0, 1.0);
        let peak = forward_peak * g * g;
        let scattering = scaled(self.mie_scattering);
        AirOptics {
            rayleigh: scaled(self.rayleigh_scattering),
            mie_scattering: scattering * (Vec3::ONE - peak),
            mie_extinction: (Vec3::from(self.mie_scattering) + Vec3::from(self.mie_absorption)) * k
                - scattering * peak,
            ozone: scaled(self.ozone_absorption),
            mie_g: g,
            mie_forward_peak: forward_peak,
        }
    }

    /// Fraction of sunlight (RGB) reaching camera-relative point `p` through the atmosphere,
    /// matching `sun_transmittance` in the shader library: through the gas (Rayleigh) and the
    /// haze or dust (Mie scattering and absorption, less a forward peak that passes straight on,
    /// [`Self::mie_forward_peak`]), times `density_scale`. It leaves out the
    /// thin ozone layer, as it always has (a few percent of Earth's noon sun).
    pub fn sun_transmittance(&self, p_rel: DVec3, center_rel: DVec3, sun_dir: Vec3) -> Vec3 {
        if !self.enabled || self.planet_radius <= 0.0 {
            return Vec3::ONE;
        }
        let air = self.optics();
        let sun = sun_dir.as_dvec3();
        let r_planet = self.planet_radius;
        let r_atmo = r_planet + self.thickness as f64;
        if let Some((t0, _)) = ray_sphere(p_rel, sun, center_rel, r_planet * 0.9995)
            && t0 > 0.0
        {
            return Vec3::ZERO;
        }
        let Some((_, t1)) = ray_sphere(p_rel, sun, center_rel, r_atmo) else {
            return Vec3::ONE;
        };
        let seg = t1.max(0.0) / 6.0;
        let (mut sr, mut sm) = (0.0f64, 0.0f64);
        for j in 0..6 {
            let q = p_rel + sun * ((j as f64 + 0.5) * seg);
            let h = ((q - center_rel).length() - r_planet).max(0.0);
            sr += (-h / self.rayleigh_scale_height as f64).exp() * seg;
            sm += (-h / self.mie_scale_height as f64).exp() * seg;
        }
        let tau = air.rayleigh * sr as f32 + air.mie_extinction * sm as f32;
        Vec3::new((-tau.x).exp(), (-tau.y).exp(), (-tau.z).exp())
    }
}

fn ray_sphere(origin: DVec3, dir: DVec3, center: DVec3, radius: f64) -> Option<(f64, f64)> {
    let oc = origin - center;
    let b = oc.dot(dir);
    let c = oc.length_squared() - radius * radius;
    let h = b * b - c;
    if h < 0.0 {
        return None;
    }
    let s = h.sqrt();
    Some((-b - s, -b + s))
}

#[cfg(test)]
mod pacing_tests {
    use super::*;

    #[test]
    fn pacing_is_off_by_default_and_at_the_pacing_rate() {
        let mut p = FramePacing::default();
        for _ in 0..200 {
            p.advance(1.0 / 120.0, 0.0);
        }
        assert_eq!(p.share, 1.0, "off: every frame does all of it");
        let mut p = FramePacing::default();
        for _ in 0..200 {
            p.advance(1.0 / 30.0, 30.0);
        }
        assert!((p.share - 1.0).abs() < 1e-3);
        for _ in 0..200 {
            p.advance(1.0 / 20.0, 30.0);
        }
        assert_eq!(
            p.share, 1.0,
            "never more than a frame's worth below the rate"
        );
    }

    #[test]
    fn a_second_holds_the_same_work_at_any_rate() {
        for fps in [60.0f32, 90.0, 120.0] {
            let mut p = FramePacing::default();
            for _ in 0..(fps as usize * 4) {
                p.advance(1.0 / fps, 30.0);
            }
            assert!(
                (p.share * fps - 30.0).abs() < 0.5,
                "{fps} fps: {} frames' worth a second",
                p.share * fps
            );
        }
        let mut p = FramePacing::default();
        for _ in 0..2000 {
            p.advance(1.0 / 500.0, 30.0);
        }
        assert_eq!(p.share, 0.25, "no less than a quarter a frame");
    }

    #[test]
    fn one_slow_frame_barely_moves_the_share() {
        let mut p = FramePacing::default();
        for _ in 0..600 {
            p.advance(1.0 / 120.0, 30.0);
        }
        let before = p.share;
        p.advance(0.1, 30.0);
        assert!(p.share < before + 0.02, "{before} -> {}", p.share);
        // A rate that stays low does bring it up, within a second or two.
        let mut t = 0.0;
        while p.share < 0.99 && t < 5.0 {
            p.advance(1.0 / 30.0, 30.0);
            t += 1.0 / 30.0;
        }
        assert!(t < 2.5, "back to a full frame's work after {t} s at 30 fps");
    }

    #[test]
    fn paced_settings_trace_less_and_draw_the_environment_less_often() {
        let base = RenderSettings::default();
        let mut s = base.clone();
        s.paced(1.0);
        assert_eq!(s, base);
        s.paced(0.5);
        assert!((s.gi_budget_ms - base.gi_budget_ms * 0.5).abs() < 1e-6);
        assert_eq!(s.environment_interval, base.environment_interval * 2);
        let mut s = base.clone();
        s.paced(0.25);
        assert_eq!(s.environment_interval, base.environment_interval * 4);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reflections_take_coarser_detail_and_cheaper_shadows() {
        let base = RenderSettings::default();
        let r = base.for_reflection();
        assert_eq!(r.contact_shadow_length, 0.0);
        assert_eq!(r.shadow_filter_taps, base.shadow_filter_taps / 2);
        let f = base.reflection_lod_distance_factor;
        assert!(f > 1.0);
        assert_eq!(r.mesh_lod_pixels, base.mesh_lod_pixels * f);
        assert_eq!(r.min_object_pixels, base.min_object_pixels * f);
        let same = RenderSettings {
            reflection_lod_distance_factor: 1.0,
            ..base.clone()
        }
        .for_reflection();
        assert_eq!(same.cluster_error_px, base.cluster_error_px);
    }

    /// Sunlight through `air` reaching a point on its planet's surface (the camera), with the sun
    /// `elevation_deg` above the horizon.
    fn sun_at_surface(air: &Atmosphere, elevation_deg: f32) -> Vec3 {
        let e = elevation_deg.to_radians();
        let center_rel = DVec3::new(0.0, -air.planet_radius, 0.0);
        air.sun_transmittance(DVec3::ZERO, center_rel, Vec3::new(0.0, e.sin(), -e.cos()))
    }

    #[test]
    fn earth_is_the_default() {
        assert_eq!(Atmosphere::earth(), Atmosphere::default());
        let air = Atmosphere::earth().optics();
        assert_eq!(air.rayleigh, Vec3::new(6.88e-6, 14.56e-6, 33.2e-6));
        assert_eq!(air.mie_scattering, Vec3::splat(3.996e-6));
        assert_eq!(air.mie_extinction, Vec3::splat(4.40e-6));
        assert_eq!(air.ozone, Vec3::new(2.67e-6, 1.87e-6, 0.149e-6));
        assert_eq!(air.mie_g, Vec3::splat(0.8));
        assert_eq!(air.mie_forward_peak, 0.0);
    }

    #[test]
    fn sun_transmittance_follows_the_air() {
        let (earth, mars) = (Atmosphere::earth(), Atmosphere::mars());
        let blue_over_red = |t: Vec3| t.z / t.x.max(1e-9);

        // Overhead, Mars' dust (optical depth about 0.34, of which its forward peak passes
        // straight on: about 0.2 is left) lets through less than Earth's air.
        let earth_noon = sun_at_surface(&earth, 90.0);
        let mars_noon = sun_at_surface(&mars, 90.0);
        eprintln!("noon: Earth {earth_noon}, Mars {mars_noon}");
        assert!(
            (0.75..0.87).contains(&mars_noon.y),
            "Mars' noon sun: {mars_noon}"
        );
        assert!(earth_noon.y > 0.85, "Earth's noon sun: {earth_noon}");

        // A low sun: Earth's air reddens it, Mars' nearly gray dust dims it without reddening.
        let earth_low = sun_at_surface(&earth, 5.0);
        let mars_low = sun_at_surface(&mars, 5.0);
        eprintln!("5° up: Earth {earth_low}, Mars {mars_low}");
        assert!(
            blue_over_red(earth_low) < 0.5,
            "Earth's low sun isn't red: {earth_low}"
        );
        assert!(
            blue_over_red(mars_low) > 0.9,
            "Mars' low sun is reddened: {mars_low}"
        );
        assert!(
            mars_low.y < earth_low.y * 0.6,
            "Mars' dust doesn't dim the low sun: {mars_low} vs Earth's {earth_low}"
        );

        // Dust that absorbs more blue lets less blue through.
        let mars_20 = sun_at_surface(&mars, 20.0);
        let blue_absorbing = Atmosphere {
            mie_absorption: [1.26e-6, 5.25e-6, 30e-6],
            ..mars
        };
        let t = sun_at_surface(&blue_absorbing, 20.0);
        assert!(t.z < mars_20.z * 0.9, "{t} vs {mars_20}");

        // The density scale scales the optical depth: twice as dense squares the transmittance;
        // no air lets everything through.
        let dense = Atmosphere {
            density_scale: 2.0,
            ..mars
        };
        let t = sun_at_surface(&dense, 20.0);
        assert!(
            (t - mars_20 * mars_20).abs().max_element() < 1e-4,
            "{t} vs {mars_20}"
        );
        let none = Atmosphere {
            density_scale: 0.0,
            ..mars
        };
        assert_eq!(sun_at_surface(&none, 20.0), Vec3::ONE);
    }

    #[test]
    fn high_quality_keeps_the_settings_and_low_cuts_the_costly_parts() {
        use engine_world::QualityPreset;
        let base = RenderSettings {
            shadow_map_size: 4096,
            ..RenderSettings::default()
        };
        assert_eq!(base.scaled(&Scalability::preset(QualityPreset::High)), base);

        let low = base.scaled(&Scalability::preset(QualityPreset::Low));
        assert!((low.render_scale - 0.67).abs() < 1e-6);
        assert_eq!(low.shadow_map_size, 2048);
        assert!(low.shadow_distance < base.shadow_distance);
        assert!(low.mesh_lod_pixels > base.mesh_lod_pixels);
        assert!(low.impostor_pixels > base.impostor_pixels);
        assert!(low.grass_density < base.grass_density);
        assert!(!low.volumetric_fog && !low.screen_space_gi && !low.screen_space_reflections);
        assert!(low.ambient_occlusion, "kept at effects 0.5");
        assert!(low.max_point_lights < base.max_point_lights);

        let ultra = base.scaled(&Scalability::preset(QualityPreset::Ultra));
        assert_eq!(ultra.shadow_map_size, 8192);
        assert!(ultra.impostor_pixels < base.impostor_pixels);
        // Nothing the game turned off comes back on.
        let off = RenderSettings {
            volumetric_fog: false,
            ..base.clone()
        };
        assert!(
            !off.scaled(&Scalability::preset(QualityPreset::Ultra))
                .volumetric_fog
        );
    }
}
