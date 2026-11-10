//! Render targets that depend on the output size.

use crate::features::Formats;
use engine_core::math::UVec2;

/// A texture with its default view.
pub struct Tex {
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    pub width: u32,
    pub height: u32,
}

impl Tex {
    pub fn new(
        device: &wgpu::Device,
        label: &str,
        width: u32,
        height: u32,
        format: wgpu::TextureFormat,
        usage: wgpu::TextureUsages,
    ) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
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
            usage,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        Self {
            texture,
            view,
            width: width.max(1),
            height: height.max(1),
        }
    }

    /// A texture of `layers` layers, its default view the whole array.
    pub fn new_layers(
        device: &wgpu::Device,
        label: &str,
        width: u32,
        height: u32,
        layers: u32,
        format: wgpu::TextureFormat,
        usage: wgpu::TextureUsages,
    ) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: width.max(1),
                height: height.max(1),
                depth_or_array_layers: layers.max(1),
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        Self {
            texture,
            view,
            width: width.max(1),
            height: height.max(1),
        }
    }

    /// A view of one layer, to draw into.
    pub fn layer_view(&self, layer: u32) -> wgpu::TextureView {
        self.texture.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2),
            base_array_layer: layer,
            array_layer_count: Some(1),
            ..Default::default()
        })
    }
}

/// Size-dependent targets.
pub struct Targets {
    /// The scene's size (every target but `ldr`).
    pub width: u32,
    pub height: u32,
    /// The output image's size (`ldr`): the window, which the scene may cover only part of.
    pub window: UVec2,
    /// Size of the anti-aliased image: the scene's, or the camera's viewport when temporal
    /// anti-aliasing also upscales (`upscaled`).
    pub output: UVec2,
    /// Scene color, pre-exposed linear HDR.
    pub hdr: Tex,
    /// Reverse-Z depth.
    pub depth: Tex,
    /// Prepass normals: octahedral world normal xy, perceptual roughness, 1 where written.
    pub prepass_normal: Tex,
    /// Prepass motion vectors: uv movement since the previous frame (current − previous).
    pub motion: Tex,
    /// Depth right after the prepass (shaders read it while the main depth is attached).
    pub prepass_depth: Tex,
    /// Raw ambient occlusion (noisy), then the blurred result shading reads.
    pub ao_raw: Tex,
    pub ao: Tex,
    /// The screen light maps (`crate::effects::Effects::encode_light`, lib_frame.wgsl
    /// `screen_light`): the sun and the sky's cover, the sky's other shares, the probes' light.
    pub light_a: Tex,
    pub light_b: Tex,
    pub light_gi: Tex,
    /// The lamps that can reach each 16-pixel tile: a bit per lamp, two layers of 128
    /// (lamp_tiles.wgsl).
    pub lamp_tiles: Tex,
    /// Temporal anti-aliasing history (`output` size), ping-ponged every frame. Without TAA, the
    /// finished HDR image is kept here for screen-space reflections.
    pub history: [Tex; 2],
    /// Temporal upscaling's result at the viewport's size, which bloom and tonemapping read in
    /// place of `hdr` (None at native resolution).
    pub upscaled: Option<Tex>,
    /// Screen-space reflections: radiance (not pre-exposed), coverage.
    pub ssr: Tex,
    /// World reflections at half resolution (probe_gi.wgsl `cs_reflect`): radiance (not
    /// pre-exposed), distance to the hit (0 = none). Screen-space reflections blend them in.
    pub world_reflection: Tex,
    /// Screen-space global illumination: this frame's rays, the temporal results (ping-ponged,
    /// view depth in alpha) and the blurred result the scene reads.
    pub gi_raw: Tex,
    pub gi_history: [Tex; 2],
    pub gi: Tex,
    pub color_copy: Tex,
    pub depth_copy: Tex,
    /// Where the transparent phase drew something thin, which TAA keeps (see effects.rs).
    pub reactive: Tex,
    /// Tonemapped display image (sRGB). Screenshots read this.
    pub ldr: Tex,
    /// Bloom mip chain (half resolution and down).
    pub bloom: Vec<Tex>,
}

/// The format of the two screen light maps that hold shares of the sun and the sky (0..1).
pub const LIGHT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
/// Pixels across a lamp tile (lamp_tiles.wgsl `LAMP_TILE`).
pub const LAMP_TILE: u32 = 16;
/// A lamp tile's bits: 128 a layer.
pub const LAMP_TILE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba32Uint;

impl Targets {
    pub fn new(
        device: &wgpu::Device,
        formats: &Formats,
        width: u32,
        height: u32,
        output: UVec2,
        window: UVec2,
    ) -> Self {
        use wgpu::TextureUsages as U;
        let width = width.max(1);
        let height = height.max(1);
        let window = window.max(UVec2::ONE);
        let output = output.max(UVec2::ONE);
        let upscale = output != UVec2::new(width, height);
        let hdr = Tex::new(
            device,
            "hdr",
            width,
            height,
            formats.hdr,
            U::RENDER_ATTACHMENT | U::TEXTURE_BINDING | U::COPY_SRC | U::COPY_DST,
        );
        let depth = Tex::new(
            device,
            "depth",
            width,
            height,
            formats.depth,
            U::RENDER_ATTACHMENT | U::TEXTURE_BINDING | U::COPY_SRC,
        );
        let screen = |label: &str, format: wgpu::TextureFormat| {
            Tex::new(
                device,
                label,
                width,
                height,
                format,
                U::RENDER_ATTACHMENT | U::TEXTURE_BINDING,
            )
        };
        let prepass_normal = screen("prepass normals", formats.normal);
        let motion = screen("motion vectors", formats.motion);
        let prepass_depth = Tex::new(
            device,
            "prepass depth",
            width,
            height,
            formats.depth,
            U::TEXTURE_BINDING | U::COPY_DST,
        );
        let ao_raw = screen("ambient occlusion raw", formats.ao);
        let ao = screen("ambient occlusion", formats.ao);
        let light_a = screen("screen light: sun and sky", LIGHT_FORMAT);
        let light_b = screen("screen light: people, leaves, reflections", LIGHT_FORMAT);
        let light_gi = screen("screen light: probes", formats.hdr);
        let lamp_tiles = Tex::new_layers(
            device,
            "lamp tiles",
            width.div_ceil(LAMP_TILE),
            height.div_ceil(LAMP_TILE),
            2,
            LAMP_TILE_FORMAT,
            U::RENDER_ATTACHMENT | U::TEXTURE_BINDING,
        );
        let history = [0, 1].map(|i| {
            Tex::new(
                device,
                &format!("taa history {i}"),
                output.x,
                output.y,
                formats.hdr,
                U::RENDER_ATTACHMENT | U::TEXTURE_BINDING | U::COPY_SRC | U::COPY_DST,
            )
        });
        let ssr = screen("screen-space reflections", formats.hdr);
        let world_reflection = Tex::new(
            device,
            "world reflections",
            width.div_ceil(2),
            height.div_ceil(2),
            wgpu::TextureFormat::Rgba16Float,
            U::STORAGE_BINDING | U::TEXTURE_BINDING | U::RENDER_ATTACHMENT,
        );
        let gi_raw = screen("screen-space gi trace", formats.hdr);
        let gi_history =
            [0, 1].map(|i| screen(&format!("screen-space gi history {i}"), formats.hdr));
        let gi = screen("screen-space gi", formats.hdr);
        let color_copy = Tex::new(
            device,
            "scene color copy",
            width,
            height,
            formats.hdr,
            U::TEXTURE_BINDING | U::COPY_DST,
        );
        let depth_copy = Tex::new(
            device,
            "scene depth copy",
            width,
            height,
            formats.depth,
            U::TEXTURE_BINDING | U::COPY_DST,
        );
        let reactive = screen("taa reactive mask", formats.ao);
        let ldr = Tex::new(
            device,
            "ldr",
            window.x,
            window.y,
            formats.ldr,
            // Extra views' pictures are copied in (`ViewTarget::Window`).
            U::RENDER_ATTACHMENT | U::TEXTURE_BINDING | U::COPY_SRC | U::COPY_DST,
        );
        let upscaled = upscale.then(|| {
            Tex::new(
                device,
                "upscaled",
                output.x,
                output.y,
                formats.hdr,
                U::TEXTURE_BINDING | U::COPY_DST,
            )
        });
        // Bloom reads the post-processing input (`post`).
        let levels = bloom_levels(output.x, output.y);
        let mut bloom = Vec::with_capacity(levels);
        let (mut w, mut h) = (output.x, output.y);
        for i in 0..levels {
            w = (w / 2).max(1);
            h = (h / 2).max(1);
            bloom.push(Tex::new(
                device,
                &format!("bloom {i}"),
                w,
                h,
                formats.hdr,
                U::RENDER_ATTACHMENT | U::TEXTURE_BINDING,
            ));
        }
        Self {
            width,
            height,
            window,
            output,
            hdr,
            depth,
            prepass_normal,
            motion,
            prepass_depth,
            ao_raw,
            ao,
            light_a,
            light_b,
            light_gi,
            lamp_tiles,
            history,
            upscaled,
            ssr,
            world_reflection,
            gi_raw,
            gi_history,
            gi,
            color_copy,
            depth_copy,
            reactive,
            ldr,
            bloom,
        }
    }
}

impl Targets {
    /// The image bloom and tonemapping read: the upscaled one, or the scene's.
    pub fn post(&self) -> &Tex {
        self.upscaled.as_ref().unwrap_or(&self.hdr)
    }
}

/// Number of bloom mips for an output size (down to roughly 8 px).
pub fn bloom_levels(width: u32, height: u32) -> usize {
    let m = width.min(height).max(1);
    ((m as f32).log2().floor() as i32 - 3).clamp(1, 7) as usize
}
