//! Materials.
//!
//! [`StandardMaterial`] is a physically based metallic/roughness material. Values are linear
//! (not sRGB). Emissive is in nits-ish units scaled by `emissive_strength`, so glowing
//! mushrooms or magic runes can bloom.

use bytemuck::{Pod, Zeroable};
use engine_assets::Asset;
use serde::{Deserialize, Serialize};

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
            params2: [self.subsurface, self.wind_sway, 0.0, 0.0],
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

/// GPU layout of a material (one entry of the material storage buffer). 80 bytes.
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct GpuMaterial {
    pub base_color: [f32; 4],
    pub emissive: [f32; 4],
    /// metallic, roughness, reflectance, alpha cutoff
    pub params: [f32; 4],
    /// subsurface, wind sway, unused, unused
    pub params2: [f32; 4],
    /// bits 0-3 alpha mode, 4 unlit, 5 double sided, 6 receives wetness
    pub flags: [u32; 4],
}
