//! Procedural PBR texture sets. A generator describes one texel at a time (albedo, height,
//! roughness, metalness, occlusion); [`TextureSet::bake`] turns that into the three images the
//! renderer wants: sRGB albedo, a tangent-space normal map derived from the height, and a
//! linear ORM map (R occlusion, G roughness, B metalness).
//!
//! Generators get UVs in 0..1 and should tile (use the `period` of the 2D noise functions).

use engine_core::prelude::*;
use engine_render::{Image, StandardMaterial};

/// One texel of a material.
#[derive(Debug, Clone, Copy)]
pub struct Texel {
    /// Linear RGB albedo 0..1.
    pub albedo: Vec3,
    /// Height in meters (relative; only differences matter).
    pub height: f32,
    pub roughness: f32,
    pub metallic: f32,
    /// Cavity occlusion 0..1 (1 = open).
    pub occlusion: f32,
}

impl Default for Texel {
    fn default() -> Self {
        Self {
            albedo: Vec3::splat(0.5),
            height: 0.0,
            roughness: 0.7,
            metallic: 0.0,
            occlusion: 1.0,
        }
    }
}

/// Baked images of one material.
pub struct TextureSet {
    pub albedo: Image,
    pub normal: Image,
    pub orm: Image,
}

fn srgb_u8(linear: f32) -> u8 {
    let c = linear.clamp(0.0, 1.0);
    let s = if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    };
    (s * 255.0 + 0.5) as u8
}

fn unorm(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}

impl TextureSet {
    /// Sample `f` on a `size`×`size` grid. `meters` is how many meters the texture spans (so
    /// heights turn into the right slopes).
    pub fn bake(size: u32, meters: f32, f: impl Fn(Vec2) -> Texel) -> Self {
        let n = size as usize;
        let mut texels = Vec::with_capacity(n * n);
        for y in 0..size {
            for x in 0..size {
                let uv = Vec2::new(
                    (x as f32 + 0.5) / size as f32,
                    (y as f32 + 0.5) / size as f32,
                );
                texels.push(f(uv));
            }
        }
        let texel_m = meters / size as f32;
        let h = |x: i64, y: i64| {
            let (x, y) = (
                x.rem_euclid(n as i64) as usize,
                y.rem_euclid(n as i64) as usize,
            );
            texels[y * n + x].height
        };
        let albedo = Image::from_fn(size, size, true, |x, y| {
            let a = texels[y as usize * n + x as usize].albedo;
            [srgb_u8(a.x), srgb_u8(a.y), srgb_u8(a.z), 255]
        });
        let normal = Image::from_fn(size, size, false, |x, y| {
            let (x, y) = (x as i64, y as i64);
            // Sobel slopes; +Y in the image goes down the texture (glTF convention: green up).
            let dx = (h(x + 1, y - 1) + 2.0 * h(x + 1, y) + h(x + 1, y + 1)
                - h(x - 1, y - 1)
                - 2.0 * h(x - 1, y)
                - h(x - 1, y + 1))
                / (8.0 * texel_m);
            let dy = (h(x - 1, y + 1) + 2.0 * h(x, y + 1) + h(x + 1, y + 1)
                - h(x - 1, y - 1)
                - 2.0 * h(x, y - 1)
                - h(x + 1, y - 1))
                / (8.0 * texel_m);
            let nrm = Vec3::new(-dx, dy, 1.0).normalize();
            [
                unorm(nrm.x * 0.5 + 0.5),
                unorm(nrm.y * 0.5 + 0.5),
                unorm(nrm.z * 0.5 + 0.5),
                255,
            ]
        });
        let orm = Image::from_fn(size, size, false, |x, y| {
            let t = texels[y as usize * n + x as usize];
            [
                unorm(t.occlusion),
                unorm(t.roughness),
                unorm(t.metallic),
                255,
            ]
        });
        let mut set = Self {
            albedo,
            normal,
            orm,
        };
        set.albedo.generate_mips();
        set.normal.generate_mips();
        set.orm.generate_mips();
        set
    }
}

/// Mix two colors.
pub fn mix(a: Vec3, b: Vec3, t: f32) -> Vec3 {
    a + (b - a) * t.clamp(0.0, 1.0)
}

/// Linear RGB from an sRGB hex color.
pub fn hex(rgb: u32) -> Vec3 {
    let m = StandardMaterial::hex(rgb);
    Vec3::new(m.base_color[0], m.base_color[1], m.base_color[2])
}

/// `smoothstep(e0, e1, x)`.
pub fn smooth(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bump_makes_a_normal_map_that_leans_away_from_it() {
        let set = TextureSet::bake(32, 1.0, |uv| Texel {
            height: if uv.x > 0.5 { 0.05 } else { 0.0 },
            ..Default::default()
        });
        // Left of the step (x ≈ 15) the surface rises to the right: the normal leans left.
        let p = set.normal.pixel(15, 10);
        assert!(p[0] < 120, "{p:?}");
        // Flat areas point straight out.
        let f = set.normal.pixel(5, 10);
        assert!((f[0] as i32 - 128).abs() <= 1 && f[2] > 250, "{f:?}");
        assert_eq!(set.orm.pixel(3, 3)[1], unorm(0.7));
    }
}
