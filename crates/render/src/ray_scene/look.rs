//! What a material looks like to a ray that hits it: one diffuse colour and its glow (a hit
//! is lit with the mean of its texture, as probe GI's rays always saw surfaces).

use std::collections::HashMap;

use engine_assets::{Assets, Handle};
use engine_core::prelude::*;

use crate::features::GpuShared;
use crate::material::StandardMaterial;

/// An RGBA8 image's mean color, linear, from about 4096 of its pixels (as the image cache
/// measures it).
pub(crate) fn image_mean(image: &crate::Image) -> Option<Vec3> {
    if image.format != crate::PixelFormat::Rgba8 || image.data.len() < 4 {
        return None;
    }
    let pixels = image.data.len() / 4;
    let step = (pixels / 4096).max(1);
    let decode = |c: u8| {
        let c = c as f32 / 255.0;
        if !image.srgb {
            c
        } else if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    let mut sum = [0.0f64; 3];
    let mut n = 0.0f64;
    for p in image.data.as_chunks::<4>().0.iter().step_by(step) {
        for c in 0..3 {
            sum[c] += decode(p[c]) as f64;
        }
        n += 1.0;
    }
    Some(Vec3::from(sum.map(|s| (s / n) as f32)))
}

/// Diffuse albedo and glow (cd/m², not pre-exposed) of a material as a ray sees it;
/// `texture_mean`: the mean color of its base color texture, when known.
pub(crate) fn material_light(m: &StandardMaterial, texture_mean: Option<Vec3>) -> (Vec3, Vec3) {
    let tint = Vec3::new(m.base_color[0], m.base_color[1], m.base_color[2]);
    let metal = m.metallic.clamp(0.0, 1.0);
    let plain = 1.0 - metal;
    let albedo = match (m.base_color_texture.is_some(), m.preset) {
        // The colour slot draws its image over a preset's map (`texture_set_for`), and a
        // preset's metallic scales its own metal map: of the image, the part over bare metal
        // is no diffuse colour. Before the image's mean is known: the preset's.
        (true, Some(p)) => match texture_mean {
            Some(mean) => tint * mean * (1.0 - metal * p.mean_metalness()),
            None => tint * Vec3::from(p.mean_albedo()),
        },
        // (Measured with the whole metal map, as a preset's material has it.)
        (false, Some(p)) => tint * Vec3::from(p.mean_albedo()),
        // A texture's mean before it is uploaded: about half of the tint.
        (true, None) => tint * texture_mean.unwrap_or(Vec3::splat(0.5)) * plain,
        (false, None) => tint * plain,
    };
    let emissive = Vec3::from(m.emissive) * m.emissive_strength;
    (albedo.min(Vec3::splat(0.95)), emissive)
}

/// The mean color of base color image `handle`: from its pixels, or (released after
/// upload) the image cache's. `means` keeps what was found, by image and version.
pub(crate) fn texture_mean(
    means: &mut HashMap<u32, (u64, Option<Vec3>)>,
    shared: &GpuShared,
    images: &Assets<crate::Image>,
    handle: Handle<crate::Image>,
) -> Option<Vec3> {
    let handle = images.canonical(handle);
    let key = handle.index();
    let version = images.version(handle);
    if let Some(&(v, mean)) = means.get(&key)
        && v == version
    {
        return mean;
    }
    let image = images.get(handle)?;
    let mean = if image.is_released() {
        // Known once it was uploaded; asked again until then.
        let mean = shared.images.mean_color(images, handle).map(Vec3::from);
        mean?;
        mean
    } else {
        image_mean(image)
    };
    means.insert(key, (version, mean));
    mean
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MaterialPreset;

    /// Of an image drawn over a preset, the part over the preset's bare metal (as far as the
    /// material's metallic lets it be metal) is no diffuse colour; without an image, the
    /// preset's own mean holds it already.
    #[test]
    fn an_image_over_a_metal_preset_is_darkened_by_its_bare_metal() {
        let mean = Vec3::splat(0.5);
        let over = |preset, metallic| {
            let m = StandardMaterial {
                base_color_texture: Some(Handle::from_index(1)),
                ..StandardMaterial::preset(preset)
            }
            .with_metallic(metallic);
            material_light(&m, Some(mean)).0
        };
        let steel = MaterialPreset::RustySteel;
        assert!(steel.mean_metalness() > 0.0, "some of rusty steel is bare");
        let bare = 0.5 * (1.0 - steel.mean_metalness());
        assert!((over(steel, 1.0) - Vec3::splat(bare)).abs().max_element() < 1e-6);
        assert!((over(steel, 0.0) - mean).abs().max_element() < 1e-6);
        // A preset without metal: the image's mean, whatever its metallic.
        let brick = MaterialPreset::Brick;
        assert_eq!(brick.mean_metalness(), 0.0);
        assert!((over(brick, 1.0) - mean).abs().max_element() < 1e-6);
        // Before the image's mean is known, and without an image: the preset's mean.
        let preset_mean = Vec3::from(steel.mean_albedo());
        let m = StandardMaterial {
            base_color_texture: Some(Handle::from_index(1)),
            ..StandardMaterial::preset(steel)
        };
        assert_eq!(material_light(&m, None).0, preset_mean);
        let m = StandardMaterial::preset(steel);
        assert_eq!(material_light(&m, Some(mean)).0, preset_mean);
    }
}
