//! Tunable render settings. All of these are registered resources, so the AI bridge, console
//! and scripts can read and patch them by name (`resource.set RenderSettings {...}`).

use engine_core::prelude::*;

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

/// Resolution of the volumetric fog grid (`RenderSettings::volumetric_fog_quality`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
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
    /// Shadows are drawn up to this distance from the camera, meters.
    pub shadow_distance: f32,
    /// Shadow map resolution per cascade and of the far shadow map (applied at startup).
    pub shadow_map_size: u32,
    /// Far shadows: beyond `shadow_distance`, big casters (terrain, mountains, cliffs, large
    /// buildings) still cast sun shadows out to this distance from the camera, meters, from one
    /// coarse shadow map (`shadow_map_size`², so 8 km gives texels of about 8 m at 2048). Mountains
    /// shade the valley at low sun and ridges shade themselves. 0 = off (no sun shadows beyond
    /// `shadow_distance`).
    pub far_shadow_distance: f32,
    /// 0 = uniform cascade splits, 1 = logarithmic.
    pub shadow_split_lambda: f32,
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
    /// Screen-space global illumination: diffuse light bounced off nearby surfaces that are on
    /// screen (sunlit walls tint the ground, lamps and emissive surfaces light their
    /// surroundings). Builds up over a few frames.
    pub screen_space_gi: bool,
    /// Strength of the bounced light (1 = physical).
    pub gi_intensity: f32,
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
    pub saturation: f32,
    pub contrast: f32,
    pub vignette: f32,
    pub film_grain: f32,
    /// Warm (+) / cool (−) tint.
    pub white_balance: f32,
    /// Maximum point lights shaded per frame (nearest first).
    pub max_point_lights: u32,
    pub debug_view: DebugView,
    /// Re-render the sky environment map every N frames (1 = every frame).
    pub environment_interval: u32,
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
    /// Cloud shadows: when a module draws the cloud shadow map (the weather's clouds; see
    /// `cloud_shadows`), the sunlight on every surface and in the fog is shaded by it. Off: the
    /// map is not drawn and nothing is shaded by clouds.
    pub cloud_shadows: bool,
    /// How many EV darker than "average gray" very dark scenes are shown (0 = a moonlit night
    /// is exposed like noon; 2.5 = night looks like night). Ramps in below EV100 ≈ 5.
    pub night_exposure_bias: f32,
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
    /// reads on the CPU or uploads itself need `Image::keep_pixels`. Off by default.
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
    /// Added to the level of detail virtual textures ask for (positive = coarser, fewer tiles).
    pub virtual_texture_mip_bias: f32,
    /// BC7-compress the cache where the GPU supports it (a quarter of the memory; encoded on the
    /// tile loader threads). Applied when changed.
    pub virtual_texture_compression: bool,
}

impl Default for RenderSettings {
    fn default() -> Self {
        Self {
            vsync: true,
            shadow_distance: 350.0,
            shadow_map_size: 2048,
            far_shadow_distance: 8000.0,
            shadow_split_lambda: 0.8,
            shadow_depth_bias: 0.0,
            shadow_normal_bias: 1.5,
            shadow_softness: 1.0,
            sun_penumbra_scale: 1.0,
            contact_shadow_length: 0.4,
            ambient_occlusion: true,
            ao_radius: 1.5,
            ao_intensity: 1.0,
            screen_space_reflections: true,
            ssr_max_roughness: 0.6,
            screen_space_gi: true,
            gi_intensity: 1.0,
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
            saturation: 1.05,
            contrast: 1.0,
            vignette: 0.2,
            film_grain: 0.004,
            white_balance: 0.0,
            max_point_lights: 256,
            debug_view: DebugView::None,
            environment_interval: 1,
            sky_cloud_layer: true,
            sky_occlusion: true,
            sky_occlusion_extent: 256.0,
            sky_occlusion_size: 512,
            cloud_shadows: true,
            night_exposure_bias: 2.5,
            night_desaturation: 0.7,
            virtual_geometry: true,
            virtual_geometry_min_triangles: 50_000,
            virtual_geometry_occlusion: true,
            virtual_geometry_cache: true,
            virtual_geometry_cache_dir: String::new(),
            virtual_geometry_cache_mb: 2048,
            cluster_error_px: 1.0,
            cluster_shadow_error_texels: 2.0,
            material_texture_size: 1024,
            release_image_pixels: false,
            virtual_textures: true,
            virtual_texture_threshold: 4096,
            virtual_texture_cache_tiles: 1024,
            virtual_texture_upload_budget: 32,
            virtual_texture_mip_bias: 0.0,
            virtual_texture_compression: true,
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
/// per meter at the surface, in the renderer's three color channels (red 680 nm, green 550 nm,
/// blue 440 nm), thinning with height over `rayleigh_scale_height` and `mie_scale_height`.
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
    /// 5.802e-6, 13.558e-6, 33.1e-6.
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
    /// RGB. Ozone absorbs orange, which keeps a twilight sky blue overhead. Earth: 0.650e-6,
    /// 1.881e-6, 0.085e-6; 0 for air without ozone.
    pub ozone_absorption: [f32; 3],
    /// Added to `mie_g` per channel (R, G, B). Dust grains a little larger than the wavelength
    /// (Mars') scatter blue into a narrower, stronger forward peak than red: a larger blue
    /// anisotropy puts a blue glow around the sun, plainest at sunset. 0 = `mie_g` for all.
    pub mie_g_offset: [f32; 3],
    /// Scales the density of the whole air, every coefficient above (1 = as given, 2 = twice as
    /// dense, 0 = no air): a season of higher pressure, a thicker variant of the same air.
    pub density_scale: f32,
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
            rayleigh_scattering: [5.802e-6, 13.558e-6, 33.1e-6],
            mie_scattering: [3.996e-6; 3],
            mie_absorption: [0.404e-6; 3],
            ozone_absorption: [0.650e-6, 1.881e-6, 0.085e-6],
            mie_g_offset: [0.0; 3],
            density_scale: 1.0,
        }
    }
}

/// The air's coefficients at the surface as the shaders use them (per meter, times
/// [`Atmosphere::density_scale`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct AirOptics {
    pub rayleigh: Vec3,
    pub mie_scattering: Vec3,
    /// Mie scattering plus absorption.
    pub mie_extinction: Vec3,
    pub ozone: Vec3,
    /// Mie anisotropy per channel.
    pub mie_g: Vec3,
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
        }
    }

    /// The coefficients the shaders get: per meter at the surface, times `density_scale`.
    pub(crate) fn optics(&self) -> AirOptics {
        let k = self.density_scale.max(0.0);
        let scaled = |c: [f32; 3]| Vec3::from(c) * k;
        let g = Vec3::splat(self.mie_g) + Vec3::from(self.mie_g_offset);
        AirOptics {
            rayleigh: scaled(self.rayleigh_scattering),
            mie_scattering: scaled(self.mie_scattering),
            mie_extinction: (Vec3::from(self.mie_scattering) + Vec3::from(self.mie_absorption)) * k,
            ozone: scaled(self.ozone_absorption),
            mie_g: g.clamp(Vec3::splat(-0.999), Vec3::splat(0.999)),
        }
    }

    /// Fraction of sunlight (RGB) reaching camera-relative point `p` through the atmosphere,
    /// matching `sun_transmittance` in the shader library: through the gas (Rayleigh) and the
    /// haze or dust (Mie scattering and absorption), times `density_scale`. It leaves out the
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
mod tests {
    use super::*;

    /// Sunlight through `air` reaching a point on its planet's surface (the camera), with the sun
    /// `elevation_deg` above the horizon.
    fn sun_at_surface(air: &Atmosphere, elevation_deg: f32) -> Vec3 {
        let e = elevation_deg.to_radians();
        let center_rel = DVec3::new(0.0, -air.planet_radius, 0.0);
        air.sun_transmittance(DVec3::ZERO, center_rel, Vec3::new(0.0, e.sin(), -e.cos()))
    }

    #[test]
    fn earth_is_the_default_with_the_old_constants() {
        assert_eq!(Atmosphere::earth(), Atmosphere::default());
        // Exactly what the shaders and the CPU used before the air became data.
        let air = Atmosphere::earth().optics();
        assert_eq!(air.rayleigh, Vec3::new(5.802e-6, 13.558e-6, 33.1e-6));
        assert_eq!(air.mie_scattering, Vec3::splat(3.996e-6));
        assert_eq!(air.mie_extinction, Vec3::splat(4.40e-6));
        assert_eq!(air.ozone, Vec3::new(0.650e-6, 1.881e-6, 0.085e-6));
        assert_eq!(air.mie_g, Vec3::splat(0.8));
    }

    #[test]
    fn sun_transmittance_follows_the_air() {
        let (earth, mars) = (Atmosphere::earth(), Atmosphere::mars());
        let blue_over_red = |t: Vec3| t.z / t.x.max(1e-9);

        // Overhead, Mars' dust (optical depth about 0.34) lets through less than Earth's air.
        let earth_noon = sun_at_surface(&earth, 90.0);
        let mars_noon = sun_at_surface(&mars, 90.0);
        eprintln!("noon: Earth {earth_noon}, Mars {mars_noon}");
        assert!(
            (0.65..0.8).contains(&mars_noon.y),
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
            mars_low.y < earth_low.y * 0.5,
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
}
