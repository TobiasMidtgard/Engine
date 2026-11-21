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
    let plain = 1.0 - m.metallic.clamp(0.0, 1.0);
    let albedo = match (m.base_color_texture.is_some(), m.preset) {
        // The colour slot draws its image over a preset's map (`texture_set_for`); a preset's
        // metallic scales its own map, so it does not darken the colour. Before the image's
        // mean is known: the preset's.
        (true, Some(p)) => tint * texture_mean.unwrap_or(Vec3::from(p.mean_albedo())),
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
