//! Materials.
//!
//! [`StandardMaterial`] is a physically based metallic/roughness material. Values are linear
//! (not sRGB). Emissive is in nits-ish units scaled by `emissive_strength`, so glowing
//! mushrooms or magic runes can bloom.
//!
//! ## Textures
//!
//! Every factor can be modulated by an optional [`Image`] (glTF conventions):
//!
//! | Field | Channels | Color space | Combined with |
//! |---|---|---|---|
//! | `base_color_texture` | RGBA | sRGB (per `Image::srgb`) | × `base_color` (alpha drives Mask/Blend) |
//! | `normal_texture` | RGB tangent-space normal, +Y up (OpenGL/glTF) | linear | strength `normal_scale` |
//! | `metallic_roughness_texture` | G = roughness, B = metallic | linear | × `perceptual_roughness`, × `metallic` |
//! | `emissive_texture` | RGB | sRGB (per `Image::srgb`) | × `emissive` × `emissive_strength` |
//! | `occlusion_texture` | R = ambient occlusion | linear | mixed by `occlusion_strength` |
//!
//! Texture coordinates are multiplied by `uv_scale` (tiling). Normal maps need no vertex
//! tangents: the shader builds the tangent frame from screen-space derivatives. In JSON a
//! texture is its handle index (`"base_color_texture": 3`); `assets.load` lists the handles
//! a file produced. Materials without textures render exactly as before and share one GPU
//! texture binding, so untextured scenes batch as well as ever.

use bytemuck::{Pod, Zeroable};
use engine_assets::{Asset, Handle};
use serde::{Deserialize, Serialize};

use crate::texture::Image;

/// How a material handles transparency.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub enum AlphaMode {
    #[default]
    Opaque,
    /// Pixels with alpha below the cutoff are discarded (leaves, fences).
    Mask(f32),
    /// Alpha-blended, drawn after opaque geometry, sorted back to front (glass, water, ghosts).
    Blend,
}

/// Physically based material (metallic/roughness workflow).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StandardMaterial {
    /// Linear RGBA albedo.
    pub base_color: [f32; 4],
    /// Linear RGB emission color.
    pub emissive: [f32; 3],
    /// Emission luminance multiplier (cd/m²-like; 1 000+ glows in daylight, 10–100 at night).
    pub emissive_strength: f32,
    /// 0 dielectric .. 1 metal.
    pub metallic: f32,
    /// 0 mirror .. 1 fully rough.
    pub perceptual_roughness: f32,
    /// Specular reflectance at normal incidence for dielectrics (0.5 ≈ 4%).
    pub reflectance: f32,
    pub alpha_mode: AlphaMode,
    pub double_sided: bool,
    /// Ignore lighting (UI-like, debug, skyboxes).
    pub unlit: bool,
    /// Wraps light around the terminator for thin, translucent things (leaves, skin, cloth).
    pub subsurface: f32,
    /// Responds to world wetness (rain darkens and adds gloss).
    pub receives_wetness: bool,
    /// Vertex sway amplitude in meters for foliage-like wind animation (0 = rigid).
    pub wind_sway: f32,
    /// RGBA texture multiplied with `base_color` (sRGB when the image is sRGB).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_color_texture: Option<Handle<Image>>,
    /// Tangent-space normal map (+Y up, glTF/OpenGL convention), linear.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub normal_texture: Option<Handle<Image>>,
    /// glTF convention: G = roughness, B = metallic, multiplied with the factors. Linear.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metallic_roughness_texture: Option<Handle<Image>>,
    /// RGB emission texture multiplied with `emissive * emissive_strength`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub emissive_texture: Option<Handle<Image>>,
    /// Ambient occlusion in the R channel (an ORM texture can serve both this and
    /// `metallic_roughness_texture`). Linear.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub occlusion_texture: Option<Handle<Image>>,
    /// Texture coordinate scale: `[4, 4]` tiles every texture four times.
    pub uv_scale: [f32; 2],
    /// Strength of the normal map's X/Y (glTF `normalTexture.scale`). 1 = as authored.
    pub normal_scale: f32,
    /// How strongly the occlusion texture darkens ambient light (glTF
    /// `occlusionTexture.strength`), 0..1.
    pub occlusion_strength: f32,
}

impl Default for StandardMaterial {
    fn default() -> Self {
        Self {
            base_color: [0.8, 0.8, 0.8, 1.0],
            emissive: [0.0; 3],
            emissive_strength: 0.0,
            metallic: 0.0,
            perceptual_roughness: 0.6,
            reflectance: 0.5,
            alpha_mode: AlphaMode::Opaque,
            double_sided: false,
            unlit: false,
            subsurface: 0.0,
            receives_wetness: true,
            wind_sway: 0.0,
            base_color_texture: None,
            normal_texture: None,
            metallic_roughness_texture: None,
            emissive_texture: None,
            occlusion_texture: None,
            uv_scale: [1.0, 1.0],
            normal_scale: 1.0,
            occlusion_strength: 1.0,
        }
    }
}

impl Asset for StandardMaterial {}

impl StandardMaterial {
    /// Opaque material with a linear RGB color.
    pub fn color(r: f32, g: f32, b: f32) -> Self {
        Self {
            base_color: [r, g, b, 1.0],
            ..Default::default()
        }
    }

    /// Opaque material from an sRGB hex color like `0x6b8e23`.
    pub fn hex(rgb: u32) -> Self {
        let c = srgb_hex_to_linear(rgb);
        Self::color(c[0], c[1], c[2])
    }

    pub fn with_roughness(mut self, r: f32) -> Self {
        self.perceptual_roughness = r;
        self
    }

    pub fn with_metallic(mut self, m: f32) -> Self {
        self.metallic = m;
        self
    }

    pub fn with_emissive(mut self, color: [f32; 3], strength: f32) -> Self {
        self.emissive = color;
        self.emissive_strength = strength;
        self
    }

    pub fn with_alpha(mut self, alpha: f32) -> Self {
        self.base_color[3] = alpha;
        self.alpha_mode = AlphaMode::Blend;
        self
    }

    pub fn with_wind_sway(mut self, meters: f32) -> Self {
        self.wind_sway = meters;
        self
    }

    pub fn with_subsurface(mut self, s: f32) -> Self {
        self.subsurface = s;
        self
    }

    /// Multiply `base_color` by a texture.
    pub fn with_base_color_texture(mut self, image: Handle<Image>) -> Self {
        self.base_color_texture = Some(image);
        self
    }

    /// Add a tangent-space normal map.
    pub fn with_normal_texture(mut self, image: Handle<Image>) -> Self {
        self.normal_texture = Some(image);
        self
    }

    /// Add a glTF metallic/roughness texture (G = roughness, B = metallic).
    pub fn with_metallic_roughness_texture(mut self, image: Handle<Image>) -> Self {
        self.metallic_roughness_texture = Some(image);
        self
    }

    /// Multiply emission by a texture.
    pub fn with_emissive_texture(mut self, image: Handle<Image>) -> Self {
        self.emissive_texture = Some(image);
        self
    }

    /// Add an ambient occlusion texture (R channel).
    pub fn with_occlusion_texture(mut self, image: Handle<Image>) -> Self {
        self.occlusion_texture = Some(image);
        self
    }

    /// Tile textures: `[2, 2]` repeats them twice in each direction.
    pub fn with_uv_scale(mut self, u: f32, v: f32) -> Self {
        self.uv_scale = [u, v];
        self
    }

    /// Cut out pixels whose alpha is below `cutoff` (in the scene and in shadows).
    pub fn with_alpha_mask(mut self, cutoff: f32) -> Self {
        self.alpha_mode = AlphaMode::Mask(cutoff);
        self
    }

    /// Texture slots in GPU binding order: base color, normal, metallic/roughness, emissive,
    /// occlusion.
    pub fn textures(&self) -> [Option<Handle<Image>>; 5] {
        [
            self.base_color_texture,
            self.normal_texture,
            self.metallic_roughness_texture,
            self.emissive_texture,
            self.occlusion_texture,
        ]
    }

    /// True if any texture slot is set.
    pub fn has_textures(&self) -> bool {
        self.textures().iter().any(Option::is_some)
    }

    /// Stable key for sharing identical inline materials.
    pub fn key(&self) -> String {
        format!("mat:{:?}", self)
    }

    pub(crate) fn to_gpu(self) -> GpuMaterial {
        let (alpha_mode, cutoff) = match self.alpha_mode {
            AlphaMode::Opaque => (0u32, 0.0),
            AlphaMode::Mask(c) => (1, c),
            AlphaMode::Blend => (2, 0.0),
        };
        let mut flags = alpha_mode;
        if self.unlit {
            flags |= 1 << 4;
        }
        if self.double_sided {
            flags |= 1 << 5;
        }
        if self.receives_wetness {
            flags |= 1 << 6;
        }
        for (i, t) in self.textures().iter().enumerate() {
            if t.is_some() {
                flags |= TEXTURE_FLAG_BASE << i;
            }
        }
        GpuMaterial {
            base_color: self.base_color,
            emissive: [
                self.emissive[0] * self.emissive_strength,
                self.emissive[1] * self.emissive_strength,
                self.emissive[2] * self.emissive_strength,
                0.0,
            ],
            params: [
                self.metallic,
                self.perceptual_roughness.clamp(0.045, 1.0),
                self.reflectance,
                cutoff,
            ],
            params2: [
                self.subsurface,
                self.wind_sway,
                self.normal_scale,
                self.occlusion_strength.clamp(0.0, 1.0),
            ],
            uv: [self.uv_scale[0], self.uv_scale[1], 0.0, 0.0],
            flags: [flags, 0, 0, 0],
        }
    }
}

/// sRGB hex (0xRRGGBB) to linear RGB.
pub fn srgb_hex_to_linear(rgb: u32) -> [f32; 3] {
    let c = |v: u32| {
        let s = (v & 0xff) as f32 / 255.0;
        if s <= 0.04045 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    };
    [c(rgb >> 16), c(rgb >> 8), c(rgb)]
}

/// `GpuMaterial::flags[0]` bit for a bound base color texture; the normal, metallic/roughness,
/// emissive and occlusion bits follow (`<< 1` … `<< 4`).
pub const TEXTURE_FLAG_BASE: u32 = 1 << 7;
/// Mask of all texture-present bits in `GpuMaterial::flags[0]`.
pub const TEXTURE_FLAGS: u32 = 0b11111 << 7;

/// GPU layout of a material (one entry of the material storage buffer, `struct Material` in
/// `mesh.wgsl` / `mesh_shadow.wgsl`). 96 bytes.
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct GpuMaterial {
    pub base_color: [f32; 4],
    pub emissive: [f32; 4],
    /// metallic, roughness, reflectance, alpha cutoff
    pub params: [f32; 4],
    /// subsurface, wind sway, normal scale, occlusion strength
    pub params2: [f32; 4],
    /// uv scale xy, unused zw
    pub uv: [f32; 4],
    /// x: bits 0-3 alpha mode, 4 unlit, 5 double sided, 6 receives wetness, 7-11 texture
    /// present (base color, normal, metallic/roughness, emissive, occlusion)
    pub flags: [u32; 4],
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_json_still_deserializes_and_untextured_gpu_data_is_unchanged() {
        let old = r#"{"base_color": [0.5, 0.25, 0.1, 1.0], "metallic": 1.0, "alpha_mode": {"Mask": 0.4}}"#;
        let m: StandardMaterial = serde_json::from_str(old).unwrap();
        assert!(!m.has_textures());
        assert_eq!(m.uv_scale, [1.0, 1.0]);
        let gpu = m.to_gpu();
        assert_eq!(gpu.flags[0] & TEXTURE_FLAGS, 0);
        assert_eq!(gpu.flags[0] & 15, 1);
        assert_eq!(gpu.params, [1.0, 0.6, 0.5, 0.4]);
        // Untextured materials serialize without texture fields.
        let v = serde_json::to_value(StandardMaterial::default()).unwrap();
        assert!(v.get("base_color_texture").is_none(), "{v}");
        assert_eq!(std::mem::size_of::<GpuMaterial>(), 96);
    }

    #[test]
    fn textures_set_flags_and_change_the_key() {
        let plain = StandardMaterial::default();
        let textured = plain
            .with_base_color_texture(Handle::from_index(3))
            .with_normal_texture(Handle::from_index(4))
            .with_uv_scale(2.0, 3.0);
        assert_ne!(plain.key(), textured.key());
        let gpu = textured.to_gpu();
        assert_eq!(
            gpu.flags[0] & TEXTURE_FLAGS,
            TEXTURE_FLAG_BASE | (TEXTURE_FLAG_BASE << 1)
        );
        assert_eq!(gpu.uv, [2.0, 3.0, 0.0, 0.0]);
        let json = serde_json::to_value(textured).unwrap();
        assert_eq!(json["base_color_texture"], 3);
        let back: StandardMaterial = serde_json::from_value(json).unwrap();
        assert_eq!(back, textured);
    }
}
