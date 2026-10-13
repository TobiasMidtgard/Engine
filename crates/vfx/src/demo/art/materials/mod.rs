//! Physically based materials for each [`Surface`], with generated texture sets.
//!
//! Every surface gets an albedo (sRGB), a tangent-space normal map and an ORM texture
//! (occlusion, roughness, metalness; it serves as both the occlusion and the
//! metallic/roughness texture), plus an emissive texture for glowing surfaces. The textures
//! are generated in code from tileable noise ([`noise`]) through height fields in meters
//! ([`textures`]): normals and occlusion come from the height, and albedo and roughness follow
//! the same structure (cavities darker and rougher, worn edges lighter, polished spots
//! smoother), the way scanned materials behave.
//!
//! Mesh UVs are meters, so `uv_scale` is `1 / tile size`: each texture covers a realistic
//! patch (2 m of paving, 50 cm of forged iron, …). Textures tile seamlessly.

pub mod noise;
pub mod textures;

mod foliage;
mod ground;
mod magic;
mod metal;
mod stone;
mod wood;

#[cfg(test)]
mod tests;

pub use magic::RuneKind;

use engine_assets::Assets;
use engine_render::{Image, StandardMaterial};

use self::textures::{Maps, TextureImages};
use super::Surface;

/// Every surface, in declaration order.
pub const SURFACES: [Surface; 20] = [
    Surface::Flagstone,
    Surface::CarvedStone,
    Surface::DarkStone,
    Surface::Rock,
    Surface::Wood,
    Surface::Iron,
    Surface::Bronze,
    Surface::Mud,
    Surface::Snow,
    Surface::Sand,
    Surface::Dirt,
    Surface::Water,
    Surface::Leaves,
    Surface::Moss,
    Surface::RuneArcane,
    Surface::RuneStorm,
    Surface::RuneLife,
    Surface::RuneEmber,
    Surface::Crystal,
    Surface::Coals,
];

/// The material for `surface`. Textures it needs are added to `images`.
pub fn make(surface: Surface, images: &mut Assets<Image>) -> StandardMaterial {
    let (material, set) = generate(surface, 0);
    attach(material, set, images)
}

/// Add a generated texture set to `images` and point `material`'s texture slots at it (the
/// ORM image serves as both occlusion and metallic/roughness texture).
pub fn attach(
    mut material: StandardMaterial,
    set: TextureImages,
    images: &mut Assets<Image>,
) -> StandardMaterial {
    let h = set.add_to(images);
    material.base_color_texture = Some(h.albedo);
    material.normal_texture = Some(h.normal);
    material.metallic_roughness_texture = Some(h.orm);
    material.occlusion_texture = Some(h.orm);
    material.emissive_texture = h.emissive;
    material
}

/// A generated texture set and the material settings that go with it.
pub struct Generated {
    pub maps: Maps,
    /// Factors and flags (emission, subsurface, alpha, …); textures and `uv_scale` are filled
    /// in when the images are added.
    pub base: StandardMaterial,
}

impl Generated {
    /// Textured material: factors at 1 so the textures carry the values.
    pub fn new(maps: Maps, base: StandardMaterial) -> Self {
        let mut base = base;
        base.base_color = [1.0, 1.0, 1.0, base.base_color[3]];
        base.metallic = 1.0;
        base.perceptual_roughness = 1.0;
        base.uv_scale = [1.0 / maps.tile.size.x, 1.0 / maps.tile.size.y];
        Self { maps, base }
    }
}

/// Generate `surface`'s material and images without adding them anywhere. `shrink` halves
/// the texture resolution that many times (tests use small textures; the look is the same,
/// with less detail). The material's texture slots are empty; [`make`] fills them.
pub fn generate(surface: Surface, shrink: u32) -> (StandardMaterial, TextureImages) {
    let g = generated(surface, shrink);
    (g.base, g.maps.images())
}

/// The texture set and settings for `surface`.
pub fn generated(surface: Surface, shrink: u32) -> Generated {
    use Surface::*;
    match surface {
        Flagstone => stone::flagstone(shrink),
        CarvedStone => stone::carved_stone(shrink),
        DarkStone => stone::dark_stone(shrink),
        Rock => stone::rock(shrink),
        Wood => wood::wood(shrink),
        Iron => metal::iron(shrink),
        Bronze => metal::bronze(shrink),
        Mud => ground::mud(shrink),
        Snow => ground::snow(shrink),
        Sand => ground::sand(shrink),
        Dirt => ground::dirt(shrink),
        Water => ground::water(shrink),
        Leaves => foliage::leaves(shrink),
        Moss => foliage::moss(shrink),
        RuneArcane => magic::rune(RuneKind::Arcane, shrink),
        RuneStorm => magic::rune(RuneKind::Storm, shrink),
        RuneLife => magic::rune(RuneKind::Life, shrink),
        RuneEmber => magic::rune(RuneKind::Ember, shrink),
        Crystal => magic::crystal(shrink),
        Coals => wood::coals(shrink),
    }
}
