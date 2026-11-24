//! What a material looks like to a ray that hits it: one diffuse colour and its glow (a hit
//! is lit with the mean of its texture, as probe GI's rays always saw surfaces), and its shine:
//! how rough it is, how much of it is bare metal and of which colour, by the means of the maps
//! the raster reads them from (a material's own metal-and-roughness image, or a preset's
//! generated maps).

use std::collections::HashMap;

use engine_assets::{Assets, Handle};
use engine_core::prelude::*;

use crate::features::GpuShared;
use crate::material::StandardMaterial;

/// An RGBA8 image's mean, linear, from about 4096 of its pixels spread over it, as the image
/// cache measures it (`image_cache::spread_mean`): as a colour (sRGB decoded when the image is
/// sRGB), or with `data` as the values a linear slot samples (a metal-and-roughness map's,
/// never decoded, whatever the image's flag says: a PNG loads flagged sRGB).
pub(crate) fn image_mean(image: &crate::Image, data: bool) -> Option<Vec3> {
    crate::image_cache::spread_mean(image, image.srgb && !data).map(Vec3::from)
}

/// The mean colour of base colour image `base` and the mean of that colour times the metal of
/// metal-and-roughness image `orm` (its blue, as data), read at the same places of both: about
/// 4096 of them, one in each cell of a grid of 64 by 64 over the images, at a place in the cell
/// a hash of the cell picks, the same place of either image whatever their sizes. Where bright
/// bare steel and dark rust share the images, the raster's mean diffuse colour,
/// `mean(c (1 - metallic b))`, is `mean(c) - metallic mean(c b)`, not the product of the two
/// images' means. `None` for other formats than RGBA8, and once either's pixels are released.
pub(crate) fn bare_colour_mean(base: &crate::Image, orm: &crate::Image) -> Option<(Vec3, Vec3)> {
    let rgba8 = |i: &crate::Image| i.format == crate::PixelFormat::Rgba8 && i.data.len() >= 4;
    if !rgba8(base) || !rgba8(orm) {
        return None;
    }
    let decode = |c: u8| {
        let c = c as f32 / 255.0;
        if !base.srgb {
            c
        } else if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    let at = |image: &crate::Image, u: f32, v: f32| {
        let (w, h) = (image.width.max(1) as usize, image.height.max(1) as usize);
        let x = ((u * w as f32) as usize).min(w - 1);
        let y = ((v * h as f32) as usize).min(h - 1);
        image.data.as_chunks::<4>().0.get(y * w + x).copied()
    };
    const CELLS: usize = 64;
    let (mut colour, mut bare, mut n) = ([0.0f64; 3], [0.0f64; 3], 0.0f64);
    for cell in 0..CELLS * CELLS {
        let hash = (cell as u64 + 1).wrapping_mul(0x9e37_79b9_7f4a_7c15);
        let hash = (hash ^ (hash >> 31)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        let u = ((cell % CELLS) as f32 + (hash as u32) as f32 / 4_294_967_296.0) / CELLS as f32;
        let v =
            ((cell / CELLS) as f32 + ((hash >> 32) as u32) as f32 / 4_294_967_296.0) / CELLS as f32;
        let (Some(c), Some(o)) = (at(base, u, v), at(orm, u, v)) else {
            continue;
        };
        let metal = o[2] as f32 / 255.0;
        for k in 0..3 {
            let c = decode(c[k]);
            colour[k] += c as f64;
            bare[k] += (c * metal) as f64;
        }
        n += 1.0;
    }
    let mean = |s: [f64; 3]| Vec3::from(s.map(|s| (s / n) as f32));
    (n > 0.0).then(|| (mean(colour), mean(bare)))
}

/// Diffuse albedo and glow (cd/m², not pre-exposed) of a material as a ray sees it;
/// `texture_mean`: the mean color of its base color texture, `orm_mean` of its metal and
/// roughness one (`metallic_roughness_texture`), when known; `pair`: the two images' own
/// [`bare_colour_mean`], where the material has both and their pixels were there. The part of
/// it that is bare metal ([`material_metallic`]) has no diffuse colour.
pub(crate) fn material_light(
    m: &StandardMaterial,
    texture_mean: Option<Vec3>,
    orm_mean: Option<Vec3>,
    pair: Option<(Vec3, Vec3)>,
) -> (Vec3, Vec3) {
    let tint = Vec3::new(m.base_color[0], m.base_color[1], m.base_color[2]);
    let plain = 1.0 - material_metallic(m, orm_mean);
    let own_pair =
        pair.filter(|_| m.base_color_texture.is_some() && m.metallic_roughness_texture.is_some());
    let albedo = match (m.base_color_texture.is_some(), m.preset, own_pair) {
        // Its own colour image and its own metal map: the colour where the map leaves it
        // diffuse, as the raster draws it texel by texel.
        (true, _, Some((colour, bare))) => {
            tint * (colour - bare * m.metallic.clamp(0.0, 1.0)).max(Vec3::ZERO)
        }
        // The colour slot draws its image in place of a preset's map (`texture_set_for`), and
        // the metal map over it (the preset's, scaled by its metallic) still makes some of it
        // bare metal: of the image, that part is no diffuse colour. Before the image's mean is
        // known: the preset's.
        (true, Some(p), None) => match texture_mean {
            Some(mean) => tint * mean * plain,
            None => tint * Vec3::from(p.mean_albedo()),
        },
        // The preset's maps in both slots: its diffuse mean, measured with the whole metal map
        // (its material's metallic 1), and the share of its steel that a lower metallic leaves
        // diffuse. Under a metal map of the material's own: the preset's colour over all of it,
        // of which that map makes its share bare metal.
        (false, Some(p), _) => match m.metallic_roughness_texture {
            None => {
                let steel = Vec3::from(p.mean_metal_colour().unwrap_or([0.0; 3]));
                let kept = (1.0 - m.metallic.clamp(0.0, 1.0)) * p.mean_metalness();
                tint * (Vec3::from(p.mean_albedo()) + steel * kept)
            }
            Some(_) => tint * Vec3::from(p.mean_base_colour()) * plain,
        },
        // A texture's mean before it is uploaded: about half of the tint. With its own metal
        // map whose pixels were gone before the two were read together: the product of their
        // means.
        (true, None, None) => tint * texture_mean.unwrap_or(Vec3::splat(0.5)) * plain,
        (false, None, _) => tint * plain,
    };
    let emissive = Vec3::from(m.emissive) * m.emissive_strength;
    (albedo.min(Vec3::splat(0.95)), emissive)
}

/// The mean of the map that a material's factor `channel` (1 roughness, 2 metalness) scales
/// in the raster (`mat.params * mr`, mesh.wgsl): its own metal-and-roughness image's (green,
/// blue; `orm_mean`), which takes a preset's map's place in that slot (`texture_set_for`), else
/// the preset's generated map's (`preset_mean`), else 1, and 1 while its own image's mean isn't
/// known (the factor alone, as the rays took it before they read the map).
fn map_mean(
    m: &StandardMaterial,
    orm_mean: Option<Vec3>,
    channel: usize,
    preset_mean: impl Fn(crate::MaterialPreset) -> f32,
) -> f32 {
    let mean = match (m.metallic_roughness_texture, orm_mean, m.preset) {
        (Some(_), Some(orm), _) => orm[channel],
        (None, _, Some(p)) => preset_mean(p),
        _ => 1.0,
    };
    mean.clamp(0.0, 1.0)
}

/// How much of a material is bare metal to a ray, 0 to 1: its metallic times the mean of the
/// metal map it scales ([`map_mean`]: of its own image, else of a preset's map,
/// `MaterialPreset::mean_metalness`: the rest of rusty steel is rust). A wooden plank drawn
/// with metallic 1 over an image whose metal is 0 is no metal.
pub(crate) fn material_metallic(m: &StandardMaterial, orm_mean: Option<Vec3>) -> f32 {
    m.metallic.clamp(0.0, 1.0) * map_mean(m, orm_mean, 2, |p| p.mean_metalness())
}

/// How rough a material is to a ray (perceptual, 0 to 1): its roughness times the mean of the
/// roughness map it scales ([`map_mean`]: of its own image, else of a preset's map,
/// `MaterialPreset::mean_roughness`).
pub(crate) fn material_roughness(m: &StandardMaterial, orm_mean: Option<Vec3>) -> f32 {
    (m.perceptual_roughness * map_mean(m, orm_mean, 1, |p| p.mean_roughness())).clamp(0.0, 1.0)
}

/// The colour of a material's bare metal to a ray (its F0, linear), and whether it follows its
/// tile's pattern as its diffuse colour does: its base colour, as the raster takes a metal's
/// base colour for its shine. Over an image, the tint times the image's mean (patterned: the
/// image is its tile); over a preset's maps alone, the tint times the preset's steel
/// (`MaterialPreset::mean_metal_colour`, which the rust's pattern doesn't follow); under a metal
/// map of the material's own over a preset's colour, the tint times the preset's mean colour
/// (`MaterialPreset::mean_base_colour`, patterned: bare where that map says, in the colour the
/// preset's map has there); else the tint. `None` where it is no metal, while its image's mean
/// isn't known and over a preset without steel of its own (grey steel's stands there,
/// `RAY_METAL_GREY` in lib_ray.wgsl).
pub(crate) fn material_metal(
    m: &StandardMaterial,
    texture_mean: Option<Vec3>,
    orm_mean: Option<Vec3>,
) -> Option<(Vec3, bool)> {
    if material_metallic(m, orm_mean) <= 0.0 {
        return None;
    }
    let tint = Vec3::new(m.base_color[0], m.base_color[1], m.base_color[2]);
    let (base, patterned) = match (m.base_color_texture, m.preset) {
        (Some(_), _) => (tint * texture_mean?, true),
        (None, Some(p)) if m.metallic_roughness_texture.is_some() => {
            (tint * Vec3::from(p.mean_base_colour()), true)
        }
        (None, Some(p)) => (tint * Vec3::from(p.mean_metal_colour()?), false),
        (None, None) => (tint, false),
    };
    Some((base.clamp(Vec3::ZERO, Vec3::ONE), patterned))
}

/// What [`texture_pair_mean`] found, by the two images (handle indices).
pub(crate) type PairMeans = HashMap<(u32, u32), PairMean>;

/// [`bare_colour_mean`] of two images, and the two images' versions and shapes when it was
/// read.
#[derive(Clone, Copy, Debug)]
pub(crate) struct PairMean {
    versions: (u64, u64),
    shapes: [(u32, u32, crate::PixelFormat); 2],
    mean: Option<(Vec3, Vec3)>,
}

/// [`bare_colour_mean`] of base colour image `base` and metal-and-roughness image `orm`, from
/// their pixels, kept once read: an image's pixels released after upload bump its version
/// (`Assets::get_mut`) but leave its picture, which the image cache tells by its shape and its
/// empty pixels, as it does to keep the image's texture (`ImageCache::get_for`). `None` where
/// either's pixels were released before the two were read together: then the product of their
/// means stands. `pairs` keeps what was found.
pub(crate) fn texture_pair_mean(
    pairs: &mut PairMeans,
    images: &Assets<crate::Image>,
    base: Handle<crate::Image>,
    orm: Handle<crate::Image>,
) -> Option<(Vec3, Vec3)> {
    let (base, orm) = (images.canonical(base), images.canonical(orm));
    let key = (base.index(), orm.index());
    let versions = (images.version(base), images.version(orm));
    let (b, o) = (images.get(base)?, images.get(orm)?);
    let shape = |i: &crate::Image| (i.width, i.height, i.format);
    if let Some(kept) = pairs.get_mut(&key) {
        let same =
            |was: u64, now: u64, image: &crate::Image, read: (u32, u32, crate::PixelFormat)| {
                was == now || (image.is_released() && shape(image) == read)
            };
        if same(kept.versions.0, versions.0, b, kept.shapes[0])
            && same(kept.versions.1, versions.1, o, kept.shapes[1])
        {
            kept.versions = versions;
            return kept.mean;
        }
    }
    let mean = bare_colour_mean(b, o);
    pairs.insert(
        key,
        PairMean {
            versions,
            shapes: [shape(b), shape(o)],
            mean,
        },
    );
    mean
}

/// The mean of image `handle`: as a colour for a base colour image, as data (`data`) for a
/// metal-and-roughness one ([`image_mean`]); from its pixels, or (released after upload) the
/// image cache's. `means` keeps what was found, by image, the way it was read and version.
pub(crate) fn texture_mean(
    means: &mut HashMap<(u32, bool), (u64, Option<Vec3>)>,
    shared: &GpuShared,
    images: &Assets<crate::Image>,
    handle: Handle<crate::Image>,
    data: bool,
) -> Option<Vec3> {
    let handle = images.canonical(handle);
    let key = (handle.index(), data);
    let version = images.version(handle);
    if let Some(&(v, mean)) = means.get(&key)
        && v == version
    {
        return mean;
    }
    let image = images.get(handle)?;
    let mean = if image.is_released() {
        // Known once it was uploaded; asked again until then.
        let mean = if data {
            shared.images.mean_data(images, handle)
        } else {
            shared.images.mean_color(images, handle)
        }
        .map(Vec3::from);
        mean?;
        mean
    } else {
        image_mean(image, data)
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
            material_light(&m, Some(mean), None, None).0
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
        assert_eq!(material_light(&m, None, None, None).0, preset_mean);
        let m = StandardMaterial::preset(steel);
        assert_eq!(material_light(&m, Some(mean), None, None).0, preset_mean);
    }

    /// A metal's colour is its base colour (gold's yellow, a texture's mean under its tint,
    /// which follows its tile's pattern), how much of it is metal its metallic; a preset's
    /// metal is as much of it as its metal map makes bare, in its steel's colour under its
    /// tint, which its rust's pattern doesn't touch.
    #[test]
    fn a_metal_keeps_its_colour_and_a_preset_its_share_of_bare_metal() {
        let gold = StandardMaterial::color(1.0, 0.77, 0.34).with_metallic(1.0);
        let gold_colour = Vec3::new(1.0, 0.77, 0.34);
        assert_eq!(
            material_metal(&gold, None, None),
            Some((gold_colour, false))
        );
        assert_eq!(material_metallic(&gold, None), 1.0);
        // Its diffuse colour is what isn't metal: none.
        assert_eq!(material_light(&gold, None, None, None).0, Vec3::ZERO);
        // Half metal: the same colour, half the share.
        let half = StandardMaterial::color(1.0, 0.77, 0.34).with_metallic(0.5);
        assert_eq!(
            material_metal(&half, None, None),
            Some((gold_colour, false))
        );
        assert_eq!(material_metallic(&half, None), 0.5);
        // No metal: no colour of it.
        let paint = StandardMaterial::color(1.0, 0.77, 0.34);
        assert_eq!(material_metal(&paint, None, None), None);
        assert_eq!(material_metallic(&paint, None), 0.0);
        // A texture's mean under the tint, once it is known, patterned as its tile.
        let textured = StandardMaterial {
            base_color_texture: Some(Handle::from_index(1)),
            ..StandardMaterial::color(0.5, 1.0, 1.0).with_metallic(1.0)
        };
        assert_eq!(material_metal(&textured, None, None), None);
        assert_eq!(
            material_metal(&textured, Some(Vec3::new(0.8, 0.6, 0.4)), None),
            Some((Vec3::new(0.4, 0.6, 0.4), true))
        );
        // A preset's metal: its map's share, its steel's colour under its tint ("dark, oily
        // steel" of the magic's chains), not patterned.
        let steel = MaterialPreset::RustySteel;
        let iron = StandardMaterial::preset(steel)
            .with_tint(0.34, 0.31, 0.3)
            .with_metallic(0.9);
        assert!((material_metallic(&iron, None) - 0.9 * steel.mean_metalness()).abs() < 1e-6);
        let (colour, patterned) = material_metal(&iron, None, None).expect("steel's colour");
        let expected = Vec3::new(0.34, 0.31, 0.3) * Vec3::from(steel.mean_metal_colour().unwrap());
        assert!((colour - expected).abs().max_element() < 1e-6, "{colour}");
        assert!(!patterned);
        // An image over the preset: the image's mean under the tint, patterned.
        let painted = StandardMaterial {
            base_color_texture: Some(Handle::from_index(1)),
            ..StandardMaterial::preset(steel)
        };
        assert_eq!(
            material_metal(&painted, Some(Vec3::splat(0.5)), None),
            Some((Vec3::splat(0.5), true))
        );
        let brick = StandardMaterial::preset(MaterialPreset::Brick).with_metallic(1.0);
        assert_eq!(material_metallic(&brick, None), 0.0);
        assert_eq!(material_metal(&brick, None, None), None);
    }

    /// A material's own metal-and-roughness image is the map its metallic and roughness scale,
    /// as the raster reads it (`mat.params * mr`): a plank drawn with metallic 1 over an image
    /// whose metal is 0 is wood to a ray, of its own colour, not black metal; one whose image
    /// is all metal is metal. Until the image's mean is known, its factors alone.
    #[test]
    fn a_metal_and_roughness_image_makes_its_share_of_metal_and_its_roughness() {
        let plank = StandardMaterial {
            base_color_texture: Some(Handle::from_index(1)),
            metallic_roughness_texture: Some(Handle::from_index(2)),
            ..StandardMaterial::color(1.0, 1.0, 1.0)
                .with_metallic(1.0)
                .with_roughness(1.0)
        };
        let wood = Vec3::new(0.4, 0.25, 0.1);
        // Occlusion 1, roughness 0.6, metal 0.
        let dielectric = Some(Vec3::new(1.0, 0.6, 0.0));
        assert_eq!(material_metallic(&plank, dielectric), 0.0);
        assert!((material_roughness(&plank, dielectric) - 0.6).abs() < 1e-6);
        assert_eq!(material_light(&plank, Some(wood), dielectric, None).0, wood);
        assert_eq!(material_metal(&plank, Some(wood), dielectric), None);
        // All metal.
        let bare = Some(Vec3::new(1.0, 0.3, 1.0));
        assert_eq!(material_metallic(&plank, bare), 1.0);
        assert_eq!(material_light(&plank, Some(wood), bare, None).0, Vec3::ZERO);
        assert_eq!(material_metal(&plank, Some(wood), bare), Some((wood, true)));
        // A quarter metal, at metallic 0.5: an eighth.
        let some = Some(Vec3::new(1.0, 0.5, 0.25));
        assert!((material_metallic(&plank.with_metallic(0.5), some) - 0.125).abs() < 1e-6);
        // Not known yet: the factors.
        assert_eq!(material_metallic(&plank, None), 1.0);
        assert_eq!(material_roughness(&plank, None), 1.0);
        // Without an image of its own, a preset's maps; without either, the factors.
        let marble = StandardMaterial::preset(MaterialPreset::Marble);
        let polish = MaterialPreset::Marble.mean_roughness();
        assert!((material_roughness(&marble, dielectric) - polish).abs() < 1e-6);
        let glossy = StandardMaterial::color(1.0, 1.0, 1.0).with_roughness(0.2);
        assert!((material_roughness(&glossy, dielectric) - 0.2).abs() < 1e-6);
        // An image of its own takes the preset's map's place: rusty steel's colour all over,
        // none of it bare where the image says no metal (not the preset's rust alone), and
        // bare in that colour, following its tile's pattern, where it says metal.
        let steel = MaterialPreset::RustySteel;
        let own = StandardMaterial {
            metallic_roughness_texture: Some(Handle::from_index(2)),
            ..StandardMaterial::preset(steel)
        };
        assert_eq!(material_metallic(&own, dielectric), 0.0);
        assert!((material_roughness(&own, dielectric) - 0.6).abs() < 1e-6);
        let all_over = Vec3::from(steel.mean_base_colour());
        assert!(
            (material_light(&own, None, dielectric, None).0 - all_over)
                .abs()
                .max_element()
                < 1e-6
        );
        assert!(
            all_over.x > steel.mean_albedo()[0],
            "the steel's share counts"
        );
        assert_eq!(material_metal(&own, None, dielectric), None);
        assert_eq!(material_light(&own, None, bare, None).0, Vec3::ZERO);
        let (colour, patterned) = material_metal(&own, None, bare).expect("bare");
        assert!((colour - all_over).abs().max_element() < 1e-6 && patterned);
        // The preset's own maps at a lower metallic: the rest of its steel is diffuse.
        let half = StandardMaterial::preset(steel).with_metallic(0.5);
        let diffuse = Vec3::from(steel.mean_albedo())
            + Vec3::from(steel.mean_metal_colour().unwrap()) * 0.5 * steel.mean_metalness();
        assert!(
            (material_light(&half, None, None, None).0 - diffuse)
                .abs()
                .max_element()
                < 1e-6
        );
        let none = StandardMaterial::preset(steel).with_metallic(0.0);
        assert!(
            (material_light(&none, None, None, None).0 - all_over)
                .abs()
                .max_element()
                < 1e-6
        );
    }

    /// A base colour image and its metal map read together: where bright bare steel and dark
    /// rust share them, the diffuse colour is what the map leaves diffuse, as the raster draws
    /// it texel by texel, not the product of the two means. The two need not be the same size.
    #[test]
    fn a_colour_image_and_its_metal_map_are_read_together() {
        // White steel on the left half, black rust on the right; the metal map twice as fine,
        // metal on its left half.
        let mut colour = Vec::new();
        for _ in 0..4 {
            for x in 0..4 {
                let c = if x < 2 { 255 } else { 0 };
                colour.extend_from_slice(&[c, c, c, 255]);
            }
        }
        let base = crate::Image::new(4, 4, colour, false);
        let mut metal = Vec::new();
        for _ in 0..8 {
            for x in 0..8 {
                metal.extend_from_slice(&[255, 128, if x < 4 { 255 } else { 0 }, 255]);
            }
        }
        let orm = crate::Image::new(8, 8, metal, true);
        let (colour, bare) = bare_colour_mean(&base, &orm).expect("RGBA8");
        assert!((colour.x - 0.5).abs() < 0.02, "{colour}");
        assert!((bare.x - 0.5).abs() < 0.02, "{bare}");
        let m = StandardMaterial {
            base_color_texture: Some(Handle::from_index(1)),
            metallic_roughness_texture: Some(Handle::from_index(2)),
            ..StandardMaterial::color(1.0, 1.0, 1.0).with_metallic(1.0)
        };
        let (mean, orm_mean) = (image_mean(&base, false), image_mean(&orm, true));
        // At metallic 1 none of it is diffuse: the white is all metal, the rest black.
        let together = material_light(&m, mean, orm_mean, Some((colour, bare))).0;
        assert!(together.max_element() < 0.02, "{together}");
        // At metallic 0.5, half the white.
        let half = material_light(&m.with_metallic(0.5), mean, orm_mean, Some((colour, bare))).0;
        assert!((half.x - 0.25).abs() < 0.02, "{half}");
        // Apart (their pixels gone before they were read together): the product of the means.
        let apart = material_light(&m, mean, orm_mean, None).0;
        assert!((apart.x - 0.25).abs() < 0.02, "{apart}");
        let mut gone = base.clone();
        gone.data.clear();
        assert_eq!(bare_colour_mean(&gone, &orm), None);
        // Read once while the pixels are there, the pair outlives their release after upload
        // (which bumps the images' versions); new pixels are read again.
        let mut images = Assets::<crate::Image>::default();
        let (b, o) = (images.add(base.clone()), images.add(orm.clone()));
        let mut pairs = PairMeans::new();
        let read = texture_pair_mean(&mut pairs, &images, b, o);
        assert_eq!(read, Some((colour, bare)));
        let before = (images.version(b), images.version(o));
        images.get_mut(b).expect("base").release_pixels();
        images.get_mut(o).expect("metal").release_pixels();
        assert_ne!((images.version(b), images.version(o)), before);
        assert_eq!(texture_pair_mean(&mut pairs, &images, b, o), read);
        let mut black = base.clone();
        black.data.fill(0);
        *images.get_mut(b).expect("base") = black;
        // (Its metal map's pixels are gone: the product of the means stands.)
        assert_eq!(texture_pair_mean(&mut pairs, &images, b, o), None);
        *images.get_mut(o).expect("metal") = orm.clone();
        let (colour, bare) = texture_pair_mean(&mut pairs, &images, b, o).expect("read again");
        assert!(
            colour.max_element() < 1e-6 && bare.max_element() < 1e-6,
            "{colour} {bare}"
        );
    }

    /// An image's mean is read from pixels spread over it, as the image cache reads it: a
    /// grille whose bars stand on the columns a stride through the rows would read (every 64th
    /// of this 1024 by 256 image) is mostly gaps. A metal-and-roughness map is read as data:
    /// loaded from a PNG it is flagged sRGB, which the raster ignores in that slot.
    #[test]
    fn an_images_mean_is_spread_over_it_and_a_maps_is_read_as_data() {
        let (w, h) = (1024u32, 256u32);
        let mut data = Vec::with_capacity((w * h * 4) as usize);
        for _ in 0..h {
            for x in 0..w {
                let c = if x % 64 < 8 { 255 } else { 0 };
                data.extend_from_slice(&[c, c, c, 255]);
            }
        }
        let grille = crate::Image::new(w, h, data, false);
        let mean = image_mean(&grille, false).expect("RGBA8");
        assert!((mean.x - 0.125).abs() < 0.03, "{mean}");
        // Green 0.6 as stored: 0.6 as data, about 0.32 decoded as a colour.
        let map = crate::Image::new(4, 4, [255, 153, 0, 255].repeat(16), true);
        let data = image_mean(&map, true).expect("RGBA8");
        assert!((data.y - 0.6).abs() < 1e-3, "{data}");
        let colour = image_mean(&map, false).expect("RGBA8");
        assert!((colour.y - 0.319).abs() < 0.01, "{colour}");
    }
}
