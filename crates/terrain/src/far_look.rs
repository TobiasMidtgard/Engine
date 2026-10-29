//! A planet's look from afar, handed to its terrain so the ground takes over from the far view
//! without the planet changing.
//!
//! Whatever draws a planet far away (the space module's far view) paints it as a map; the
//! terrain draws the same planet up close from its own ground. Without a shared picture, a
//! ship closing in sees the planet swap its face where the terrain appears. Games that use
//! both (as Kerbal Space Program's scaled-space maps are exported from its terrain, or
//! SpaceEngine draws one surface function at every distance) put a [`FarLook`] on the
//! terrain's entity: where a pixel spans kilometers, the ground wears the map's colors and
//! glow; nearer, the map tints the ground's own colors at the scale of whole provinces, so
//! the dark plains and bright dunes seen from orbit are still there on the way down.

use std::sync::Arc;

use engine_core::prelude::*;

/// Width of the map the renderer keeps (height is half). Larger maps are filtered down to it.
pub const FAR_LOOK_WIDTH: u32 = 1024;

/// A planet's colors from afar (a component next to its [`PlanetTerrain`]).
///
/// [`PlanetTerrain`]: crate::PlanetTerrain
#[derive(Component, Clone)]
pub struct FarLook {
    pub map: Arc<FarLookMap>,
    /// How strongly the map tints the ground's own colors up close (0..1): high where the map
    /// holds what the ground lacks (provinces, dune seas, the ice's stained lineae), low where
    /// the ground draws its own (a living world's biomes).
    pub near_tint: f32,
}

/// An equirectangular map of a planet: rows from the north pole down, columns from longitude
/// −180° eastward, in the terrain's own frame (+Y north, longitude 0 toward +Z, 90° east
/// toward +X).
///
/// Made with [`FarLookMap::new`], which filters the map to the renderer's size and its mips
/// at once (best on the thread that painted it), so drawing it only uploads.
#[derive(Debug, Clone)]
pub struct FarLookMap {
    /// [`FAR_LOOK_WIDTH`] × half and each mip below it, RGBA as given to `new`.
    levels: Vec<Vec<u8>>,
    /// Mean linear albedo over the sphere.
    mean: [f32; 3],
}

impl std::fmt::Debug for FarLook {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "FarLook(near tint {})", self.near_tint)
    }
}

impl FarLookMap {
    /// A map `width`×`height` (2:1). `rgba`: albedo sRGB-encoded in RGB, rows from the north
    /// pole, and in A where molten rock shows through (0..1, linear), lit at the palette's
    /// `lava_glow`.
    pub fn new(width: u32, height: u32, rgba: Vec<u8>) -> Self {
        assert_eq!(
            rgba.len(),
            (width * height * 4) as usize,
            "far look map size"
        );
        let src = Source {
            width,
            height,
            rgba,
        };
        let mean = src.mean_albedo();
        let mut levels = vec![src.resized()];
        let (mut w, mut h) = (FAR_LOOK_WIDTH, FAR_LOOK_WIDTH / 2);
        while h > 1 {
            let next = halved(levels.last().expect("level"), w, h);
            levels.push(next);
            (w, h) = (w / 2, h / 2);
        }
        Self { levels, mean }
    }

    /// The map at [`FAR_LOOK_WIDTH`] × half, then each mip down to 2×1.
    pub fn levels(&self) -> &[Vec<u8>] {
        &self.levels
    }

    /// Mean linear albedo over the sphere (each row weighted by its area).
    pub fn mean_albedo(&self) -> [f32; 3] {
        self.mean
    }
}

/// A far look map as given.
struct Source {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

impl Source {
    /// The map at the renderer's size: box-filtered down (or repeated up) to
    /// [`FAR_LOOK_WIDTH`] × half, in linear light.
    fn resized(&self) -> Vec<u8> {
        let (w, h) = (FAR_LOOK_WIDTH as usize, FAR_LOOK_WIDTH as usize / 2);
        let (sw, sh) = (self.width as usize, self.height as usize);
        if sw == w && sh == h {
            return self.rgba.clone();
        }
        let fx = (sw / w).max(1);
        let fy = (sh / h).max(1);
        let mut out = vec![0u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                let (x0, y0) = (x * sw / w, y * sh / h);
                let mut sum = [0.0f32; 4];
                for j in 0..fy {
                    for i in 0..fx {
                        let s = ((y0 + j).min(sh - 1) * sw + (x0 + i).min(sw - 1)) * 4;
                        add_texel(&mut sum, &self.rgba[s..s + 4]);
                    }
                }
                let o = (y * w + x) * 4;
                put_mean(&mut out[o..o + 4], sum, (fx * fy) as f32);
            }
        }
        out
    }

    fn mean_albedo(&self) -> [f32; 3] {
        let (w, h) = (self.width as usize, self.height as usize);
        let (mut sum, mut weight) = ([0.0f64; 3], 0.0f64);
        let step = (w / 128).max(1);
        for y in 0..h {
            let lat =
                std::f64::consts::FRAC_PI_2 - (y as f64 + 0.5) / h as f64 * std::f64::consts::PI;
            let a = lat.cos();
            for x in (0..w).step_by(step) {
                let s = (y * w + x) * 4;
                for (c, v) in sum.iter_mut().enumerate() {
                    *v += a * srgb_to_linear(self.rgba[s + c]) as f64;
                }
                weight += a;
            }
        }
        sum.map(|v| (v / weight.max(1e-9)) as f32)
    }
}

/// Planets with a far look drawn at once (layers of the renderer's map array).
pub(crate) const FAR_LOOK_LAYERS: u32 = 4;

/// The far looks on the GPU: one layer of a texture array per planet that has one.
pub(crate) struct FarLookPages {
    texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    /// Per layer: the planet and the map uploaded there, and when it was last drawn.
    layers: Vec<Option<(Entity, Arc<FarLookMap>, u64)>>,
    /// Mean albedo per layer.
    means: Vec<[f32; 3]>,
    frame: u64,
}

impl FarLookPages {
    pub fn new(device: &wgpu::Device) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("terrain far looks"),
            size: wgpu::Extent3d {
                width: FAR_LOOK_WIDTH,
                height: FAR_LOOK_WIDTH / 2,
                depth_or_array_layers: FAR_LOOK_LAYERS,
            },
            mip_level_count: FAR_LOOK_WIDTH.ilog2(),
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        Self {
            texture,
            view,
            layers: vec![None; FAR_LOOK_LAYERS as usize],
            means: vec![[0.0; 3]; FAR_LOOK_LAYERS as usize],
            frame: 0,
        }
    }

    pub fn begin_frame(&mut self) {
        self.frame += 1;
    }

    /// The layer holding `look` for `planet` (uploading it into the least recently drawn
    /// layer if it isn't there yet) and the map's mean albedo; `None` when every layer was
    /// drawn this frame.
    pub fn layer(
        &mut self,
        queue: &wgpu::Queue,
        planet: Entity,
        look: &FarLook,
    ) -> Option<(u32, [f32; 3])> {
        let frame = self.frame;
        if let Some(i) = self.layers.iter().position(|l| {
            l.as_ref()
                .is_some_and(|(e, m, _)| *e == planet && Arc::ptr_eq(m, &look.map))
        }) {
            if let Some(l) = self.layers[i].as_mut() {
                l.2 = frame;
            }
            return Some((i as u32, self.means[i]));
        }
        let i = (0..self.layers.len())
            .filter(|&i| self.layers[i].as_ref().is_none_or(|l| l.2 < frame))
            .min_by_key(|&i| {
                self.layers[i]
                    .as_ref()
                    // A layer of this planet's older map first, then free ones, then the oldest.
                    .map_or(0, |l| if l.0 == planet { 0 } else { 1 + l.2 })
            })?;
        let (mut w, mut h) = (FAR_LOOK_WIDTH, FAR_LOOK_WIDTH / 2);
        for (mip, level) in look.map.levels().iter().enumerate() {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.texture,
                    mip_level: mip as u32,
                    origin: wgpu::Origin3d {
                        x: 0,
                        y: 0,
                        z: i as u32,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                level,
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
            (w, h) = ((w / 2).max(1), (h / 2).max(1));
        }
        self.means[i] = look.map.mean_albedo();
        self.layers[i] = Some((planet, look.map.clone(), frame));
        Some((i as u32, self.means[i]))
    }
}

/// `level` (`w`×`h`) at half the size, 2×2 texels averaged in linear light.
fn halved(level: &[u8], w: u32, h: u32) -> Vec<u8> {
    let (w, h, sw) = (w as usize / 2, h as usize / 2, w as usize);
    let mut out = vec![0u8; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            let mut sum = [0.0f32; 4];
            for (i, j) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let s = ((2 * y + j) * sw + 2 * x + i) * 4;
                add_texel(&mut sum, &level[s..s + 4]);
            }
            let o = (y * w + x) * 4;
            put_mean(&mut out[o..o + 4], sum, 4.0);
        }
    }
    out
}

/// Add an RGBA texel to `sum`: color in linear light, alpha as is (0..255).
fn add_texel(sum: &mut [f32; 4], texel: &[u8]) {
    for (c, (v, &t)) in sum.iter_mut().zip(texel).enumerate() {
        *v += if c < 3 { srgb_to_linear(t) } else { t as f32 };
    }
}

/// Write the mean of `n` texels summed by [`add_texel`].
fn put_mean(out: &mut [u8], sum: [f32; 4], n: f32) {
    for (c, (o, v)) in out.iter_mut().zip(sum).enumerate() {
        *o = if c < 3 {
            linear_to_srgb(v / n)
        } else {
            (v / n + 0.5) as u8
        };
    }
}

fn srgb_to_linear(v: u8) -> f32 {
    static LUT: std::sync::OnceLock<[f32; 256]> = std::sync::OnceLock::new();
    LUT.get_or_init(|| {
        std::array::from_fn(|i| {
            let s = i as f32 / 255.0;
            if s <= 0.04045 {
                s / 12.92
            } else {
                ((s + 0.055) / 1.055).powf(2.4)
            }
        })
    })[v as usize]
}

fn linear_to_srgb(l: f32) -> u8 {
    let s = if l <= 0.003_130_8 {
        l * 12.92
    } else {
        1.055 * l.powf(1.0 / 2.4) - 0.055
    };
    (s.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resized_keeps_the_mean_and_the_glow() {
        // A 2048×1024 map, left half black, right half mid grey, glowing in its north.
        let (w, h) = (2048u32, 1024u32);
        let mut rgba = vec![0u8; (w * h * 4) as usize];
        for y in 0..h {
            for x in 0..w {
                let i = ((y * w + x) * 4) as usize;
                let v = if x >= w / 2 { 188 } else { 0 };
                rgba[i..i + 3].copy_from_slice(&[v, v, v]);
                rgba[i + 3] = if y < h / 2 { 255 } else { 0 };
            }
        }
        let map = FarLookMap::new(w, h, rgba);
        assert_eq!(map.levels().len(), FAR_LOOK_WIDTH.ilog2() as usize);
        assert_eq!(map.levels().last().unwrap().len(), 2 * 4);
        let small = &map.levels()[0];
        assert_eq!(
            small.len(),
            (FAR_LOOK_WIDTH * FAR_LOOK_WIDTH / 2 * 4) as usize
        );
        let at = |x: u32, y: u32| {
            let i = ((y * FAR_LOOK_WIDTH + x) * 4) as usize;
            [small[i], small[i + 3]]
        };
        assert_eq!(at(10, 10), [0, 255]);
        assert_eq!(at(FAR_LOOK_WIDTH - 10, 500), [188, 0]);
        let mean = map.mean_albedo();
        assert!(
            (mean[0] - 0.5 * srgb_to_linear(188)).abs() < 0.01,
            "{mean:?}"
        );
    }
}
