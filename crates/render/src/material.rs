//! Materials.
//!
//! [`StandardMaterial`] is a physically based metallic/roughness material. Values are linear
//! (not sRGB). Emissive is in nits-ish units scaled by `emissive_strength`, so glowing
//! mushrooms or magic runes can bloom.
//!
//! ## Presets
//!
//! [`StandardMaterial::preset`] gives a ready-made surface whose textures the GPU generates on
//! first use (see [`material_library`](crate::material_library)): brick, cobblestone, rock,
//! concrete, plaster, wood planks, bark, rusty steel, painted metal, sand, forest soil, gravel,
//! snow, marble and grass. Tune it with `with_texture_scale` (meters per tile), `with_tint`
//! and `with_detail`, and map it without UVs with `with_triplanar` / `with_triplanar_world`.
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
//! | `height_texture` | R = height, 0 deepest .. 1 top | linear | relief of `parallax_depth` meters |
//!
//! A preset fills every slot left `None` with its generated map, so a custom normal map can
//! replace just the preset's.
//!
//! Plain or textured, a material can add procedural surface detail (`with_detail`): bumps,
//! mottled color and roughness from 3D noise, anchored to the object (or the world).
//!
//! ## Mapping, parallax
//!
//! With [`TextureMapping::Uv`] (the default) texture coordinates are multiplied by `uv_scale`
//! and divided by `texture_scale`, reading UV units as meters (as `Shape::Plane`,
//! `Shape::RoundedCuboid` and most architecture exports lay them out). Triplanar mapping projects
//! the textures along three axes in meters instead, blending the projections by the normal and
//! reorienting the normal maps (Mikkelsen's surface gradients), so spheres, rocks and terrain
//! show no stretching and no seams. A height map adds parallax occlusion mapping: texels shift
//! with the view as if the relief were really there, and the relief shadows itself in low sun.
//!
//! Normal maps need no vertex tangents: the shader builds the tangent frame from screen-space
//! derivatives. In JSON a texture is its handle index (`"base_color_texture": 3`);
//! `assets.load` lists the handles a file produced. Materials without textures render exactly as
//! before and share one GPU texture binding, so untextured scenes batch as well as ever.

use bytemuck::{Pod, Zeroable};
use engine_assets::{Asset, Handle};
use serde::{Deserialize, Serialize};

use crate::material_library::MaterialPreset;
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

/// How textures are laid onto a surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum TextureMapping {
    /// The mesh's texture coordinates, x `uv_scale` / `texture_scale` (UV units read as meters).
    #[default]
    Uv,
    /// Triplanar in object space: projected along the object's three axes in meters (its scale
    /// included) and blended by the normal. Needs no UVs, never stretches, and moves with the
    /// object (rocks, props, spheres, primitives).
    Triplanar,
    /// Triplanar in world space: seamless across meshes split into pieces (terrain, cliffs
    /// built from several rocks, walls of many blocks), but stays put when the object moves.
    TriplanarWorld,
    /// `projection` with anti-tiling: the texture is laid on a grid of hexagons, each showing it
    /// at a random offset (and with `rotate`, a random rotation: for textures without a
    /// direction, such as ground, moss or rock, not bark or planks), blended where they meet
    /// with weights that follow the texture's brightness and a variance-preserving blend that
    /// keeps its contrast. Only the detail is hex-tiled: the texture's slow variation (a darker
    /// corner, an overall slope in the normal map) is evened out to its mean, so hexagons don't
    /// show as light and dark patches. A texture repeated over a large surface then shows no
    /// repeating pattern. Costs three samples per map instead of one (nine with triplanar), plus
    /// three low-pass samples for the base color and the normal map.
    AntiTiled {
        projection: TextureProjection,
        rotate: bool,
    },
}

/// How [`TextureMapping::AntiTiled`] projects the texture before hex-tiling it (the plain
/// mappings of the same names).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum TextureProjection {
    #[default]
    Uv,
    Triplanar,
    TriplanarWorld,
}

impl TextureMapping {
    /// The projection (anti-tiled or not).
    pub fn projection(self) -> TextureProjection {
        match self {
            TextureMapping::Uv => TextureProjection::Uv,
            TextureMapping::Triplanar => TextureProjection::Triplanar,
            TextureMapping::TriplanarWorld => TextureProjection::TriplanarWorld,
            TextureMapping::AntiTiled { projection, .. } => projection,
        }
    }
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
    /// Responds to the weather on the ground where it stands (`SurfaceLayers`, per pixel):
    /// rain darkens a rough, porous surface and glosses a smooth, sealed one (porosity follows
    /// roughness), snow settles on top faces, fire scorch chars it.
    pub receives_wetness: bool,
    /// Vertex sway amplitude in meters for foliage-like wind animation (0 = rigid), growing with
    /// height in the mesh. Each vertex scales it by its color's alpha, its wind weight (0 = still,
    /// such as a trunk's base; 1 = full sway, the default); on swaying materials the vertex
    /// alpha is not opacity.
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
    /// Height map in the R channel (0 = deepest, 1 = top) for parallax occlusion mapping: the
    /// surface looks `parallax_depth` meters deep. Linear.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height_texture: Option<Handle<Image>>,
    /// A generated material whose maps fill every texture slot left `None`
    /// ([`StandardMaterial::preset`] also sets the fitting scale, relief and detail).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preset: Option<MaterialPreset>,
    /// How textures are mapped onto the surface (mesh UVs or triplanar projection).
    pub mapping: TextureMapping,
    /// Meters covered by one texture tile (UV mapping reads UV units as meters). Presets start
    /// at their real-world tile size (`MaterialPreset::tile_size`).
    pub texture_scale: f32,
    /// Depth of the height map's relief, meters (0 = no parallax). Presets start at
    /// `MaterialPreset::relief_depth`.
    pub parallax_depth: f32,
    /// With a height map, the relief casts shadows onto itself toward the sun (deep mortar
    /// joints and the gaps between cobbles stay dark in low sun).
    pub parallax_shadows: bool,
    /// Texture coordinate scale: `[4, 4]` tiles every texture four times.
    pub uv_scale: [f32; 2],
    /// Strength of the normal map's X/Y (glTF `normalTexture.scale`). 1 = as authored.
    pub normal_scale: f32,
    /// How strongly the occlusion texture darkens ambient light (glTF
    /// `occlusionTexture.strength`), 0..1.
    pub occlusion_strength: f32,
    /// Procedural surface detail, 0 (off) .. 1: fine bumps, mottled color and roughness from
    /// 3D noise, so plain-colored surfaces (stone, plaster, bark, soil, metal plates) don't look
    /// flat. Works without textures and on top of them.
    pub detail: f32,
    /// Size of the detail's bumps in meters (its mottling is about 8 times larger).
    pub detail_scale: f32,
    /// Anchor the detail to the world instead of the object: for terrain and other meshes split
    /// into pieces whose detail must line up across the seams. Object-anchored detail (the
    /// default) moves with the object.
    pub detail_in_world: bool,
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
            height_texture: None,
            preset: None,
            mapping: TextureMapping::Uv,
            texture_scale: 1.0,
            parallax_depth: 0.0,
            parallax_shadows: true,
            uv_scale: [1.0, 1.0],
            normal_scale: 1.0,
            occlusion_strength: 1.0,
            detail: 0.0,
            detail_scale: 0.25,
            detail_in_world: false,
        }
    }
}

impl Asset for StandardMaterial {}

impl StandardMaterial {
    /// A ready-made surface whose textures (base color, normal, roughness/metalness, ambient
    /// occlusion and height) are generated on the GPU the first time it is drawn: see
    /// [`MaterialPreset`]. It starts at the preset's real-world tile size with parallax and a
    /// touch of large-scale variation against visible tiling:
    ///
    /// ```ignore
    /// let wall = StandardMaterial::preset(MaterialPreset::Brick);
    /// let boulder = StandardMaterial::preset(MaterialPreset::Rock).with_triplanar();
    /// let floor = StandardMaterial::preset(MaterialPreset::WoodPlanks).with_tint(1.0, 0.85, 0.7);
    /// ```
    pub fn preset(preset: MaterialPreset) -> Self {
        let tile = preset.tile_size();
        let (detail, subsurface) = match preset {
            MaterialPreset::Marble | MaterialPreset::PaintedMetal => (0.12, 0.0),
            MaterialPreset::Snow => (0.3, 0.25),
            MaterialPreset::Grass => (0.35, 0.15),
            _ => (0.3, 0.0),
        };
        Self {
            base_color: [1.0; 4],
            metallic: 1.0,
            perceptual_roughness: 1.0,
            subsurface,
            preset: Some(preset),
            texture_scale: tile,
            parallax_depth: preset.relief_depth(),
            parallax_shadows: true,
            detail,
            detail_scale: tile * 0.4,
            ..Default::default()
        }
    }

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

    /// Make one texture tile cover `meters`. The relief scales with it (`parallax_depth`), so
    /// bricks twice as large have joints twice as deep.
    pub fn with_texture_scale(mut self, meters: f32) -> Self {
        let meters = meters.max(1e-4);
        if self.texture_scale > 0.0 {
            self.parallax_depth *= meters / self.texture_scale;
        }
        self.texture_scale = meters;
        self
    }

    /// Multiply the base color by a linear RGB tint (keeps alpha): tint a preset's brick redder
    /// or its wood darker. 1, 1, 1 leaves it as generated.
    pub fn with_tint(mut self, r: f32, g: f32, b: f32) -> Self {
        self.base_color = [r, g, b, self.base_color[3]];
        self
    }

    /// Tint with an sRGB hex color (`0xffffff` leaves the color as is).
    pub fn with_tint_hex(self, rgb: u32) -> Self {
        let c = srgb_hex_to_linear(rgb);
        self.with_tint(c[0], c[1], c[2])
    }

    /// Choose how textures are mapped (see [`TextureMapping`]).
    pub fn with_mapping(mut self, mapping: TextureMapping) -> Self {
        self.mapping = mapping;
        self
    }

    /// Triplanar mapping in object space: no UVs needed, no stretching, moves with the object.
    pub fn with_triplanar(self) -> Self {
        self.with_mapping(TextureMapping::Triplanar)
    }

    /// Triplanar mapping in world space: seamless across neighboring meshes (terrain).
    pub fn with_triplanar_world(self) -> Self {
        self.with_mapping(TextureMapping::TriplanarWorld)
    }

    /// Anti-tiling on top of the current mapping (see [`TextureMapping::AntiTiled`]); `rotate`
    /// also turns each hexagon, for textures without a direction.
    pub fn with_anti_tiling(self, rotate: bool) -> Self {
        let projection = self.mapping.projection();
        self.with_mapping(TextureMapping::AntiTiled { projection, rotate })
    }

    /// Add a height map (R channel) for parallax occlusion mapping, `depth` meters deep.
    pub fn with_height_texture(mut self, image: Handle<Image>, depth: f32) -> Self {
        self.height_texture = Some(image);
        self.parallax_depth = depth;
        self
    }

    /// Set the parallax relief depth in meters (0 turns parallax off).
    pub fn with_parallax(mut self, depth: f32) -> Self {
        self.parallax_depth = depth.max(0.0);
        self
    }

    /// Turn off the relief's self-shadowing toward the sun.
    pub fn without_parallax_shadows(mut self) -> Self {
        self.parallax_shadows = false;
        self
    }

    /// Add procedural surface detail: `strength` 0..1, bumps about `scale` meters across.
    pub fn with_detail(mut self, strength: f32, scale: f32) -> Self {
        self.detail = strength;
        self.detail_scale = scale;
        self
    }

    /// Anchor the surface detail to the world (seamless across meshes split into pieces).
    pub fn with_detail_in_world(mut self) -> Self {
        self.detail_in_world = true;
        self
    }

    /// Cut out pixels whose alpha is below `cutoff` (in the scene and in shadows).
    pub fn with_alpha_mask(mut self, cutoff: f32) -> Self {
        self.alpha_mode = AlphaMode::Mask(cutoff);
        self
    }

    /// Texture slots in GPU binding order: base color, normal, metallic/roughness, emissive,
    /// occlusion, height.
    pub fn textures(&self) -> [Option<Handle<Image>>; 6] {
        [
            self.base_color_texture,
            self.normal_texture,
            self.metallic_roughness_texture,
            self.emissive_texture,
            self.occlusion_texture,
            self.height_texture,
        ]
    }

    /// True if any texture slot is set or a preset provides the textures.
    pub fn has_textures(&self) -> bool {
        self.preset.is_some() || self.textures().iter().any(Option::is_some)
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
        if self.detail_in_world {
            flags |= DETAIL_IN_WORLD_FLAG;
        }
        flags |= match self.mapping.projection() {
            TextureProjection::Uv => 0,
            TextureProjection::Triplanar => 1,
            TextureProjection::TriplanarWorld => 2,
        } << MAPPING_SHIFT;
        if let TextureMapping::AntiTiled { rotate, .. } = self.mapping {
            flags |= ANTI_TILING_FLAG;
            if rotate {
                flags |= ANTI_TILING_ROTATE_FLAG;
            }
        }
        if self.parallax_shadows {
            flags |= PARALLAX_SHADOWS_FLAG;
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
            uv: [
                self.uv_scale[0],
                self.uv_scale[1],
                self.detail.clamp(0.0, 1.0),
                self.detail_scale.max(1e-3),
            ],
            mapping: [
                1.0 / self.texture_scale.max(1e-4),
                self.parallax_depth.max(0.0),
                TRIPLANAR_SHARPNESS,
                0.0,
            ],
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
/// emissive, occlusion and height bits follow (`<< 1` … `<< 5`).
pub const TEXTURE_FLAG_BASE: u32 = 1 << 7;
/// Mask of all texture-present bits in `GpuMaterial::flags[0]`.
pub const TEXTURE_FLAGS: u32 = 0b111111 << 7;
/// `GpuMaterial::flags[0]` bit: surface detail anchored to the world.
pub const DETAIL_IN_WORLD_FLAG: u32 = 1 << 13;
/// `GpuMaterial::flags[0]` bits 14-15: texture mapping (0 UV, 1 triplanar, 2 triplanar world).
pub const MAPPING_SHIFT: u32 = 14;
/// `GpuMaterial::flags[0]` bit: parallax self-shadowing toward the sun.
pub const PARALLAX_SHADOWS_FLAG: u32 = 1 << 16;
/// `GpuMaterial::flags[0]` bit: hex-tiled ([`TextureMapping::AntiTiled`]).
pub const ANTI_TILING_FLAG: u32 = 1 << 17;
/// `GpuMaterial::flags[0]` bit: each hexagon also turned.
pub const ANTI_TILING_ROTATE_FLAG: u32 = 1 << 18;
/// Exponent sharpening the triplanar blend weights (higher = narrower transitions).
const TRIPLANAR_SHARPNESS: f32 = 6.0;

/// GPU layout of a material (one entry of the material storage buffer, `struct Material` in
/// `mesh.wgsl` / `mesh_shadow.wgsl`). 112 bytes.
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct GpuMaterial {
    pub base_color: [f32; 4],
    pub emissive: [f32; 4],
    /// metallic, roughness, reflectance, alpha cutoff
    pub params: [f32; 4],
    /// subsurface, wind sway, normal scale, occlusion strength
    pub params2: [f32; 4],
    /// uv scale xy, detail strength, detail scale (m)
    pub uv: [f32; 4],
    /// tiles per meter (1 / texture scale), parallax depth (m), triplanar sharpness, unused
    pub mapping: [f32; 4],
    /// x: bits 0-3 alpha mode, 4 unlit, 5 double sided, 6 receives wetness, 7-12 texture
    /// present (base color, normal, metallic/roughness, emissive, occlusion, height), 13 detail
    /// in world, 14-15 mapping, 16 parallax shadows
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
        assert!(v.get("preset").is_none(), "{v}");
        assert_eq!(std::mem::size_of::<GpuMaterial>(), 112);
        assert_eq!(gpu.mapping, [1.0, 0.0, TRIPLANAR_SHARPNESS, 0.0]);
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
        assert_eq!(gpu.uv, [2.0, 3.0, 0.0, 0.25]);
        let json = serde_json::to_value(textured).unwrap();
        assert_eq!(json["base_color_texture"], 3);
        let back: StandardMaterial = serde_json::from_value(json).unwrap();
        assert_eq!(back, textured);
    }

    #[test]
    fn presets_scale_their_relief_and_round_trip() {
        let brick = StandardMaterial::preset(MaterialPreset::Brick);
        assert!(brick.has_textures());
        assert_eq!(brick.texture_scale, MaterialPreset::Brick.tile_size());
        assert_eq!(brick.parallax_depth, MaterialPreset::Brick.relief_depth());
        let big = brick.with_texture_scale(brick.texture_scale * 2.0);
        assert!((big.parallax_depth - 2.0 * brick.parallax_depth).abs() < 1e-6);
        let gpu = big.with_triplanar_world().to_gpu();
        assert_eq!((gpu.flags[0] >> MAPPING_SHIFT) & 3, 2);
        assert_ne!(gpu.flags[0] & PARALLAX_SHADOWS_FLAG, 0);
        assert!((gpu.mapping[0] * big.texture_scale - 1.0).abs() < 1e-6);
        // Presets are data: JSON names them, and a tint is the base color.
        let json = serde_json::to_value(brick.with_tint(1.0, 0.5, 0.5)).unwrap();
        assert_eq!(json["preset"], "Brick");
        assert_eq!(json["base_color"][1], 0.5);
        let back: StandardMaterial = serde_json::from_value(json).unwrap();
        assert_eq!(back.preset, Some(MaterialPreset::Brick));
        assert_ne!(
            brick.key(),
            StandardMaterial::preset(MaterialPreset::Rock).key()
        );
    }
}
