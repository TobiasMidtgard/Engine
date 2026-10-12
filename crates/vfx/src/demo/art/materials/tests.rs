use engine_core::math::Vec3;

use super::*;

/// Small textures keep the tests quick; the generators work in meters, so the result is the
/// same material with less detail.
const SHRINK: u32 = 3;

fn all_images(set: &TextureImages) -> Vec<(&'static str, &Image)> {
    let mut v = vec![
        ("albedo", &set.albedo),
        ("normal", &set.normal),
        ("orm", &set.orm),
    ];
    if let Some(e) = &set.emissive {
        v.push(("emissive", e));
    }
    v
}

#[test]
fn every_surface_makes_a_textured_material() {
    let mut images = Assets::<Image>::default();
    for s in SURFACES {
        let (m, set) = generate(s, SHRINK);
        for (name, img) in all_images(&set) {
            assert!(img.is_valid(), "{s:?} {name} image is malformed");
        }
        let m = attach(m, set, &mut images);
        assert!(m.base_color_texture.is_some() && m.normal_texture.is_some());
        assert_eq!(m.metallic_roughness_texture, m.occlusion_texture);
        assert!(
            m.uv_scale[0] > 0.2 && m.uv_scale[0] < 5.0,
            "{s:?} {:?}",
            m.uv_scale
        );
        let glows = matches!(
            s,
            Surface::RuneArcane
                | Surface::RuneStorm
                | Surface::RuneLife
                | Surface::RuneEmber
                | Surface::Crystal
        );
        assert_eq!(m.emissive_texture.is_some(), glows, "{s:?}");
        assert_eq!(m.emissive_strength > 0.0, glows, "{s:?}");
    }
}

#[test]
fn make_adds_full_size_textures() {
    let mut images = Assets::<Image>::default();
    let m = make(Surface::Sand, &mut images);
    let albedo = images.get(m.base_color_texture.unwrap()).unwrap();
    assert_eq!((albedo.width, albedo.height), (512, 512));
    assert!(albedo.srgb);
    assert!(!images.get(m.normal_texture.unwrap()).unwrap().srgb);
}

/// Mean absolute difference between two columns (or rows) of an image, all channels.
fn line_diff(img: &Image, a: u32, b: u32, columns: bool) -> f32 {
    let n = if columns { img.height } else { img.width };
    let mut sum = 0.0;
    for k in 0..n {
        let (pa, pb) = if columns {
            (img.pixel(a, k), img.pixel(b, k))
        } else {
            (img.pixel(k, a), img.pixel(k, b))
        };
        for c in 0..4 {
            sum += (pa[c] as f32 - pb[c] as f32).abs();
        }
    }
    sum / (n * 4) as f32
}

/// The step from the last line of an image to the first should look like any step inside it.
/// Paving joints, plank gaps and carved grooves may legitimately sit on the edge, so the edge
/// is compared with the lines around it (a feature there is a few lines wide) and with the
/// sharpest step anywhere in the image, besides the image's average.
#[test]
fn textures_tile_without_seams() {
    let mut seams = Vec::new();
    for s in SURFACES {
        let (_, set) = generate(s, SHRINK - 1);
        for (name, img) in all_images(&set) {
            for columns in [true, false] {
                let len = if columns { img.width } else { img.height };
                // steps[k]: from line k to line k + 1, wrapping (the last one is the edge).
                let steps: Vec<f32> = (0..len)
                    .map(|k| line_diff(img, k, (k + 1) % len, columns))
                    .collect();
                let n = steps.len();
                let seam = steps[n - 1];
                let inner = &steps[..n - 1];
                let mean = inner.iter().sum::<f32>() / inner.len() as f32;
                let sharpest = inner.iter().copied().fold(0.0f32, f32::max);
                // Steps two and three lines away (the next ones over would share a jump at the
                // edge through the normals' filter).
                let near = [steps[1], steps[2], steps[n - 4], steps[n - 3]]
                    .into_iter()
                    .fold(0.0f32, f32::max);
                let allowed = (mean * 2.0 + 1.5)
                    .max(near * 2.0 + 2.0)
                    .min(sharpest * 1.1 + 1.0);
                if seam > allowed {
                    seams.push(format!(
                        "{s:?} {name}: the {} edges jump by {seam:.2} (mean {mean:.2}, \
                         nearby {near:.2}, sharpest {sharpest:.2})",
                        if columns { "left/right" } else { "top/bottom" },
                    ));
                }
            }
        }
    }
    assert!(seams.is_empty(), "{}", seams.join("\n"));
}

#[test]
fn normal_maps_hold_unit_vectors() {
    for s in SURFACES {
        let (_, set) = generate(s, SHRINK);
        let img = &set.normal;
        for y in 0..img.height {
            for x in 0..img.width {
                let p = img.pixel(x, y);
                let v = [p[0], p[1], p[2]].map(|c| c as f32 / 255.0 * 2.0 - 1.0);
                let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
                assert!((len - 1.0).abs() < 0.02, "{s:?} ({x}, {y}): length {len}");
                assert!(v[2] > 0.0, "{s:?} ({x}, {y}) points into the surface");
            }
        }
    }
}

#[test]
fn generation_is_deterministic() {
    for s in SURFACES {
        let (m1, a) = generate(s, SHRINK);
        let (m2, b) = generate(s, SHRINK);
        assert_eq!(m1, m2);
        for ((name, x), (_, y)) in all_images(&a).into_iter().zip(all_images(&b)) {
            assert!(x == y, "{s:?} {name} differs between runs");
        }
    }
}

#[test]
fn albedo_stays_physically_plausible() {
    for s in SURFACES {
        let g = generated(s, SHRINK);
        let mean = g.maps.albedo.iter().copied().sum::<Vec3>() / g.maps.albedo.len() as f32;
        let lum = mean.dot(Vec3::new(0.2126, 0.7152, 0.0722));
        assert!(lum > 0.02 && lum < 0.9, "{s:?}: mean albedo {mean}");
        // Water's base color is a transmission tint, not a diffuse albedo.
        if s == Surface::Water {
            continue;
        }
        for (i, c) in g.maps.albedo.iter().enumerate() {
            if g.maps.metal[i] < 0.5 {
                assert!(
                    c.min_element() >= 0.015 && c.max_element() <= 0.93,
                    "{s:?}: dielectric albedo {c} at {i}"
                );
            }
        }
    }
}
