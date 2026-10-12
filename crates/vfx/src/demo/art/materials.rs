//! Physically based materials for each [`Surface`].
//!
//! Stand-ins for now: flat colors with sensible roughness and metalness. Generated textures
//! (albedo, normal, roughness/metal, occlusion) replace them surface by surface.

use engine_assets::Assets;
use engine_render::{Image, StandardMaterial};

use super::Surface;

/// The material for `surface`. Textures it needs are added to `images`.
pub fn make(surface: Surface, _images: &mut Assets<Image>) -> StandardMaterial {
    use Surface::*;
    let rune = |rgb: [f32; 3], strength: f32| {
        StandardMaterial::hex(0x1c1a22)
            .with_roughness(0.5)
            .with_emissive(rgb, strength)
    };
    match surface {
        Flagstone => StandardMaterial::hex(0x6a645c).with_roughness(0.8),
        CarvedStone => StandardMaterial::hex(0x9a938a).with_roughness(0.75),
        DarkStone => StandardMaterial::hex(0x34302e).with_roughness(0.7),
        Rock => StandardMaterial::hex(0x6f6a66).with_roughness(0.85),
        Wood => StandardMaterial::hex(0x6e4a2c).with_roughness(0.7),
        Iron => StandardMaterial::hex(0x55585c)
            .with_metallic(1.0)
            .with_roughness(0.45),
        Bronze => StandardMaterial::hex(0x8c6a3a)
            .with_metallic(1.0)
            .with_roughness(0.4),
        Mud => StandardMaterial::hex(0x3d2b1f).with_roughness(0.35),
        Snow => StandardMaterial::hex(0xf2f4f7)
            .with_roughness(0.6)
            .with_subsurface(0.3),
        Sand => StandardMaterial::hex(0xc9b28a).with_roughness(0.9),
        Dirt => StandardMaterial::hex(0x5c4632).with_roughness(0.9),
        Water => {
            let mut m = StandardMaterial::hex(0x1d4a55)
                .with_roughness(0.04)
                .with_alpha(0.88);
            m.reflectance = 0.5;
            m
        }
        Leaves => StandardMaterial::hex(0x2f4a24)
            .with_roughness(0.8)
            .with_subsurface(0.4),
        Moss => StandardMaterial::hex(0x3c5a26).with_roughness(0.9),
        RuneArcane => rune([0.7, 0.45, 1.0], 60.0),
        RuneStorm => rune([0.55, 0.75, 1.0], 60.0),
        RuneLife => rune([0.5, 1.0, 0.55], 60.0),
        RuneEmber => rune([1.0, 0.5, 0.2], 60.0),
        Crystal => StandardMaterial::hex(0x8fb4ff)
            .with_roughness(0.1)
            .with_emissive([0.55, 0.75, 1.0], 30.0),
    }
}
