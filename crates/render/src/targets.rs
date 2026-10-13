//! Render targets that depend on the output size.

use crate::features::Formats;

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
}

/// Size-dependent targets.
pub struct Targets {
    pub width: u32,
    pub height: u32,
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
    /// Temporal anti-aliasing history, ping-ponged every frame. Without TAA, the finished HDR
    /// image is kept here for screen-space reflections.
    pub history: [Tex; 2],
    /// Screen-space reflections: radiance (not pre-exposed), coverage.
    pub ssr: Tex,
    /// Screen-space global illumination: this frame's rays, the temporal results (ping-ponged,
    /// view depth in alpha) and the blurred result the scene reads.
    pub gi_raw: Tex,
    pub gi_history: [Tex; 2],
    pub gi: Tex,
    pub color_copy: Tex,
    pub depth_copy: Tex,
    /// Tonemapped display image (sRGB). Screenshots read this.
    pub ldr: Tex,
    /// Bloom mip chain (half resolution and down).
    pub bloom: Vec<Tex>,
}

impl Targets {
    pub fn new(device: &wgpu::Device, formats: &Formats, width: u32, height: u32) -> Self {
        use wgpu::TextureUsages as U;
        let width = width.max(1);
        let height = height.max(1);
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
        let history = [0, 1].map(|i| {
            Tex::new(
                device,
                &format!("taa history {i}"),
                width,
                height,
                formats.hdr,
                U::RENDER_ATTACHMENT | U::TEXTURE_BINDING | U::COPY_SRC | U::COPY_DST,
            )
        });
        let ssr = screen("screen-space reflections", formats.hdr);
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
        let ldr = Tex::new(
            device,
            "ldr",
            width,
            height,
            formats.ldr,
            U::RENDER_ATTACHMENT | U::TEXTURE_BINDING | U::COPY_SRC,
        );
        let levels = bloom_levels(width, height);
        let mut bloom = Vec::with_capacity(levels);
        let (mut w, mut h) = (width, height);
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
            hdr,
            depth,
            prepass_normal,
            motion,
            prepass_depth,
            ao_raw,
            ao,
            history,
            ssr,
            gi_raw,
            gi_history,
            gi,
            color_copy,
            depth_copy,
            ldr,
            bloom,
        }
    }
}

/// Number of bloom mips for an output size (down to roughly 8 px).
pub fn bloom_levels(width: u32, height: u32) -> usize {
    let m = width.min(height).max(1);
    ((m as f32).log2().floor() as i32 - 3).clamp(1, 7) as usize
}
