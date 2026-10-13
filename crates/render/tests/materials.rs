//! Procedural material presets and the mapping features they use: every preset generates
//! finite, physically plausible, tileable maps; parallax occlusion mapping shifts texels the
//! right way; triplanar mapping wraps a sphere without seams; and a preset draws through the
//! ordinary mesh pipeline. Needs a GPU or lavapipe; passes with a note when neither is
//! available.

use std::path::Path;

use engine_assets::{Assets, Handle};
use engine_core::prelude::*;
use engine_render::prelude::*;
use engine_render::{MaterialMaps, RenderDisabled, Renderer, capture_now};
use image::RgbaImage;

/// Generated map size for the tests (the default is 1024; the generators are resolution
/// independent, so a smaller size checks the same shapes faster).
const MAP_SIZE: u32 = 256;

fn headless_app(width: u32, height: u32, configure: impl FnOnce(&mut RenderSettings)) -> App {
    engine_core::log::init("error");
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = width;
        w.height = height;
    }
    app.add_plugins(RenderPlugin);
    {
        let mut s = app.world.resource_mut::<RenderSettings>();
        s.tonemapper = Tonemapper::None;
        s.bloom_intensity = 0.0;
        s.vignette = 0.0;
        s.film_grain = 0.0;
        s.saturation = 1.0;
        s.sharpness = 0.0;
        s.anti_aliasing = AntiAliasing::None;
        s.material_texture_size = MAP_SIZE;
        configure(&mut s);
    }
    app
}

/// Render a few frames and read the picture back; `None` (with a note) without a GPU. Panics
/// if the GPU reported a validation error.
fn render(app: &mut App, name: &str) -> Option<RgbaImage> {
    app.update_n(4);
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping {name}: no GPU ({})", d.0);
        return None;
    }
    let path = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}.png"));
    capture_now(&mut app.world, &path).expect("screenshot");
    let gpu_errors: Vec<String> = engine_core::log::log_buffer()
        .tail(64, Some("error"))
        .into_iter()
        .filter(|l| l.message.contains("GPU error"))
        .map(|l| l.message)
        .collect();
    assert!(
        gpu_errors.is_empty(),
        "GPU errors while rendering {name}: {gpu_errors:#?}"
    );
    Some(image::open(&path).expect("png").to_rgba8())
}

fn add_image(app: &mut App, image: Image) -> Handle<Image> {
    app.world.resource_mut::<Assets<Image>>().add(image)
}

fn luma(p: &image::Rgba<u8>) -> f32 {
    0.2126 * p[0] as f32 + 0.7152 * p[1] as f32 + 0.0722 * p[2] as f32
}

fn srgb_to_linear(c: u8) -> f32 {
    let s = c as f32 / 255.0;
    if s <= 0.04045 {
        s / 12.92
    } else {
        ((s + 0.055) / 1.055).powf(2.4)
    }
}

fn percentile(values: &mut [f32], p: f32) -> f32 {
    values.sort_by(f32::total_cmp);
    let i = ((values.len() - 1) as f32 * p).round() as usize;
    values[i]
}

/// Per-texel statistics of one preset's maps.
struct MapStats {
    /// Linear albedo luminance of non-metal texels.
    dielectric_albedo: Vec<f32>,
    /// Linear albedo luminance of texels with metalness above 0.9.
    metal_albedo: Vec<f32>,
    metal_fraction: f32,
    max_metallic: f32,
    roughness: Vec<f32>,
    min_normal_z: f32,
    mean_normal_z: f32,
    mean_ao: f32,
    /// Share of height texels pinned at 0 or 1 (a crushed height range).
    height_clipped: f32,
    height_range: f32,
}

fn stats(maps: &MaterialMaps) -> MapStats {
    let n = maps.base_color.width;
    let mut s = MapStats {
        dielectric_albedo: Vec::new(),
        metal_albedo: Vec::new(),
        metal_fraction: 0.0,
        max_metallic: 0.0,
        roughness: Vec::new(),
        min_normal_z: 1.0,
        mean_normal_z: 0.0,
        mean_ao: 0.0,
        height_clipped: 0.0,
        height_range: 0.0,
    };
    let (mut hmin, mut hmax) = (255u8, 0u8);
    let mut metals = 0usize;
    let mut clipped = 0usize;
    for y in 0..n {
        for x in 0..n {
            let c = maps.base_color.pixel(x, y);
            let lum = 0.2126 * srgb_to_linear(c[0])
                + 0.7152 * srgb_to_linear(c[1])
                + 0.0722 * srgb_to_linear(c[2]);
            let orm = maps.orm.pixel(x, y);
            let metallic = orm[2] as f32 / 255.0;
            s.max_metallic = s.max_metallic.max(metallic);
            if metallic > 0.9 {
                s.metal_albedo.push(lum);
                metals += 1;
            } else if metallic < 0.1 {
                s.dielectric_albedo.push(lum);
            }
            s.roughness.push(orm[1] as f32 / 255.0);
            s.mean_ao += orm[0] as f32 / 255.0;
            let nz = maps.normal.pixel(x, y)[2] as f32 / 127.5 - 1.0;
            s.min_normal_z = s.min_normal_z.min(nz);
            s.mean_normal_z += nz;
            let h = maps.height.pixel(x, y)[0];
            hmin = hmin.min(h);
            hmax = hmax.max(h);
            if h == 0 || h == 255 {
                clipped += 1;
            }
        }
    }
    let texels = (n * n) as f32;
    s.metal_fraction = metals as f32 / texels;
    s.mean_ao /= texels;
    s.mean_normal_z /= texels;
    s.height_clipped = clipped as f32 / texels;
    s.height_range = (hmax - hmin) as f32 / 255.0;
    s
}

/// How much a map jumps across its wrap-around edges compared with inside: about 1 for a map
/// that tiles seamlessly, much more for one with a seam.
fn seam_ratio(img: &Image, channel: usize) -> f32 {
    let n = img.width;
    let v = |x: u32, y: u32| img.pixel(x % n, y % n)[channel] as f32;
    let (mut seam, mut inside) = (0.0, 0.0);
    for i in 0..n {
        seam += (v(n - 1, i) - v(0, i)).abs() + (v(i, n - 1) - v(i, 0)).abs();
        let m = n / 2;
        inside += (v(m - 1, i) - v(m, i)).abs() + (v(i, m - 1) - v(i, m)).abs();
    }
    (seam + n as f32) / (inside + n as f32)
}

#[test]
fn every_preset_generates_plausible_tileable_maps() {
    let mut app = headless_app(64, 64, |_| {});
    app.update();
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return;
    }
    let renderer = app.world.resource::<Renderer>();
    let mut failures = Vec::new();
    for preset in MaterialPreset::ALL {
        let maps = renderer
            .shared
            .read_material_maps(preset, MAP_SIZE)
            .expect("generate");
        let name = preset.name();
        let mut s = stats(&maps);
        let mut check = |ok: bool, what: String| {
            if !ok {
                failures.push(format!("{name}: {what}"));
            }
        };
        check(
            maps.non_finite_texels == 0,
            format!("{} non-finite texels", maps.non_finite_texels),
        );
        check(
            maps.clamped_texels == 0,
            format!("{} base color texels outside 0..1", maps.clamped_texels),
        );
        // Albedo: real dielectrics reflect about 2-90% of light (charcoal to fresh snow).
        let lo = percentile(&mut s.dielectric_albedo, 0.01);
        let hi = percentile(&mut s.dielectric_albedo, 0.99);
        let median = percentile(&mut s.dielectric_albedo, 0.5);
        check(
            lo >= 0.008 && hi <= 0.92,
            format!("dielectric albedo 1%..99% = {lo:.3}..{hi:.3}"),
        );
        if preset == MaterialPreset::Snow {
            check(median >= 0.7, format!("snow median albedo {median:.3}"));
        } else {
            check(
                (0.02..=0.8).contains(&median),
                format!("median albedo {median:.3}"),
            );
        }
        // Metals: the metal presets show bare metal (bright, F0 above 0.45); the rest has none.
        if preset.has_metal() {
            check(
                s.metal_fraction >= 0.03,
                format!("only {:.1}% metal texels", s.metal_fraction * 100.0),
            );
            let dull = percentile(&mut s.metal_albedo, 0.05);
            check(dull >= 0.4, format!("metal albedo 5% = {dull:.3}"));
        } else {
            check(
                s.max_metallic <= 0.02,
                format!("metalness up to {:.2} on a non-metal", s.max_metallic),
            );
        }
        let r_lo = percentile(&mut s.roughness, 0.01);
        let r_median = percentile(&mut s.roughness, 0.5);
        check(r_lo >= 0.02, format!("roughness 1% = {r_lo:.3}"));
        if preset == MaterialPreset::Marble {
            check(
                r_median <= 0.2,
                format!("polished marble roughness {r_median:.2}"),
            );
        }
        check(
            s.min_normal_z > 0.0 && s.mean_normal_z > 0.6,
            format!(
                "normals z min {:.2} mean {:.2}",
                s.min_normal_z, s.mean_normal_z
            ),
        );
        check(
            (0.4..=1.0).contains(&s.mean_ao),
            format!("mean occlusion {:.2}", s.mean_ao),
        );
        check(
            s.height_range >= 0.1 && s.height_clipped <= 0.05,
            format!(
                "height range {:.2}, {:.1}% clipped",
                s.height_range,
                s.height_clipped * 100.0
            ),
        );
        for (map, image, channel) in [
            ("base color", &maps.base_color, 1),
            ("normal", &maps.normal, 0),
            ("height", &maps.height, 0),
        ] {
            let ratio = seam_ratio(image, channel);
            check(ratio < 2.0, format!("{map} seam ratio {ratio:.2}"));
        }
        eprintln!(
            "{name}: albedo {lo:.3}/{median:.3}/{hi:.3}, roughness {r_lo:.2}/{r_median:.2}, \
             metal {:.1}%, ao {:.2}, normal z {:.2}, height range {:.2}",
            s.metal_fraction * 100.0,
            s.mean_ao,
            s.mean_normal_z,
            s.height_range
        );
    }
    assert!(
        failures.is_empty(),
        "implausible maps:\n{}",
        failures.join("\n")
    );
    // Each preset was generated once (at the one size asked for) and is cached.
    let generated = app
        .world
        .resource::<Renderer>()
        .shared
        .materials
        .generated();
    assert_eq!(generated.len(), MaterialPreset::ALL.len(), "{generated:?}");
    assert!(generated.iter().all(|&(_, size)| size == MAP_SIZE));
}

/// A floor seen from 1.5 m away at about 18 degrees, textured with a dark far half and a bright
/// near half that meet at z = 0 (the middle of the screen), with a flat height map at the
/// bottom of `depth` meters of relief.
fn stripe_floor(depth: f32) -> App {
    let mut app = headless_app(160, 160, |_| {});
    app.world.spawn((
        Camera {
            exposure: Exposure::Manual { ev100: 14.0 },
            ..Camera::perspective(50.0)
        },
        Transform::from_xyz(0.0, 0.5, 1.5).looking_at(DVec3::ZERO, Vec3::Y),
    ));
    app.world.spawn((
        DirectionalLight::default(),
        Transform::default().looking_at(DVec3::new(0.2, -1.0, -0.3), Vec3::Y),
    ));
    // Image rows (v) 0..32 dark, 32..64 bright; one tile covers 4 m, so on the 20 m floor
    // (u, v in meters from a corner) the halves meet at z = 0.
    let stripes = add_image(
        &mut app,
        Image::from_fn(64, 64, true, |_, y| {
            if y < 32 {
                [30, 30, 30, 255]
            } else {
                [220, 220, 220, 255]
            }
        }),
    );
    let bottom = add_image(&mut app, Image::from_fn(4, 4, false, |_, _| [0, 0, 0, 255]));
    let mut floor = StandardMaterial::color(1.0, 1.0, 1.0)
        .with_roughness(0.9)
        .with_base_color_texture(stripes)
        .with_texture_scale(4.0)
        .with_height_texture(bottom, depth)
        .without_parallax_shadows();
    if depth == 0.0 {
        floor.height_texture = None;
    }
    app.world.spawn((
        Transform::default(),
        MeshRenderer::new(
            Shape::Plane {
                size: [20.0, 20.0],
                subdivisions: 1,
            },
            floor,
        ),
    ));
    app
}

/// Screen row where the center column turns from dark (the floor just beyond z = 0) to bright
/// (the floor in front of it), searching down from the dark part.
fn edge_row(img: &RgbaImage) -> f32 {
    let x = img.width() / 2;
    let rows: Vec<f32> = (0..img.height())
        .map(|y| {
            (x - 2..=x + 2)
                .map(|x| luma(img.get_pixel(x, y)))
                .sum::<f32>()
                / 5.0
        })
        .collect();
    // Row 40% down sees the floor about 0.6 m beyond the edge, row 7/8 down about 0.8 m before.
    let dark_row = img.height() as usize * 2 / 5;
    let (dark, bright) = (rows[dark_row], rows[img.height() as usize * 7 / 8]);
    assert!(
        bright > dark + 30.0,
        "stripes visible: {dark:.0} vs {bright:.0}"
    );
    let mid = (dark + bright) * 0.5;
    let y = (dark_row
        + rows[dark_row..]
            .iter()
            .position(|&l| l > mid)
            .expect("an edge")) as f32;
    // Sub-pixel: interpolate between the rows around the crossing.
    let (a, b) = (rows[y as usize - 1], rows[y as usize]);
    y - 1.0 + (mid - a) / (b - a).max(1e-3)
}

#[test]
fn parallax_shifts_texels_away_from_the_viewer() {
    let Some(flat) = render(&mut stripe_floor(0.0), "parallax_flat") else {
        return;
    };
    let Some(deep) = render(&mut stripe_floor(0.1), "parallax_deep") else {
        return;
    };
    let (flat_row, deep_row) = (edge_row(&flat), edge_row(&deep));
    // Relief 10 cm deep under the surface: each pixel sees the floor of the relief, which lies
    // further along its view ray, so it shows texels from further away (about 0.1 / tan 18° =
    // 0.3 m). The dark-to-bright edge therefore appears closer to the camera: lower on screen.
    let shift = deep_row - flat_row;
    eprintln!("edge row {flat_row:.1} flat, {deep_row:.1} with 10 cm of parallax");
    assert!(
        shift > 3.0,
        "edge at row {flat_row:.1} flat and {deep_row:.1} with parallax: expected it to move \
         down (toward the viewer) by several pixels"
    );
    // Not wildly more than the geometry allows (0.3 m at this distance is about 10 px).
    assert!(shift < 25.0, "edge moved {shift:.1} px");
}

/// A sphere with a smooth tiling color pattern and gentle normal-mapped bumps, seen from the
/// front. `mapping` decides how the textures wrap it; the UV sphere's texture seam (u = 0/1)
/// faces the camera.
fn pattern_sphere(mapping: TextureMapping, texture_scale: f32) -> App {
    let mut app = headless_app(200, 200, |_| {});
    app.world.spawn((
        Camera {
            exposure: Exposure::Manual { ev100: 14.0 },
            ..Camera::perspective(50.0)
        },
        Transform::from_xyz(0.0, 0.0, 3.0).looking_at(DVec3::ZERO, Vec3::Y),
    ));
    app.world.spawn((
        DirectionalLight::default(),
        Transform::default().looking_at(DVec3::new(0.4, -0.5, -1.0), Vec3::Y),
    ));
    let tau = std::f32::consts::TAU;
    let color = add_image(
        &mut app,
        Image::from_fn(64, 64, true, |x, y| {
            let (a, b) = (x as f32 / 64.0 * tau, y as f32 / 64.0 * tau);
            let v = (150.0 + 60.0 * a.sin() * b.cos()) as u8;
            [v, v, v, 255]
        }),
    );
    let bumps = add_image(
        &mut app,
        Image::from_fn(64, 64, false, |x, y| {
            let (a, b) = (x as f32 / 64.0 * tau * 2.0, y as f32 / 64.0 * tau * 2.0);
            let n = Vec3::new(0.25 * a.cos(), 0.25 * b.sin(), 1.0).normalize();
            let c = |v: f32| ((v * 0.5 + 0.5) * 255.0).round() as u8;
            [c(n.x), c(n.y), c(n.z), 255]
        }),
    );
    let material = StandardMaterial::color(1.0, 1.0, 1.0)
        .with_roughness(0.9)
        .with_base_color_texture(color)
        .with_normal_texture(bumps)
        .with_texture_scale(texture_scale)
        .with_mapping(mapping);
    app.world.spawn((
        Transform::default().with_rotation(Quat::from_rotation_y(-std::f32::consts::FRAC_PI_2)),
        MeshRenderer::new(
            Shape::Sphere {
                radius: 1.0,
                segments: 96,
            },
            material,
        ),
    ));
    app
}

/// Pixels inside the sphere (away from its outline) whose brightness jumps by more than a
/// smooth pattern and smooth lighting can explain: seams.
fn seam_pixels(img: &RgbaImage) -> usize {
    let (w, h) = (img.width() as f32, img.height() as f32);
    // Sphere of radius 1 at 3 m, 50° field of view.
    let radius_px = (1.0f32 / 8.0f32.sqrt()).atan() / (25.0f32.to_radians()).tan() * h * 0.5;
    let mut count = 0;
    for y in 1..img.height() - 1 {
        for x in 1..img.width() - 1 {
            let (dx, dy) = (x as f32 - w * 0.5, y as f32 - h * 0.5);
            if (dx * dx + dy * dy).sqrt() > radius_px * 0.8 {
                continue;
            }
            let l = luma(img.get_pixel(x, y));
            let jump = (l - luma(img.get_pixel(x + 1, y)))
                .abs()
                .max((l - luma(img.get_pixel(x, y + 1))).abs());
            if jump > 24.0 {
                count += 1;
            }
        }
    }
    count
}

#[test]
fn triplanar_mapping_wraps_a_sphere_without_seams() {
    // Control: UV mapping with a texture scale that doesn't divide the sphere's u range shows a
    // hard seam where u wraps, so the metric can see one.
    let Some(uv) = render(
        &mut pattern_sphere(TextureMapping::Uv, 0.37),
        "triplanar_control_uv",
    ) else {
        return;
    };
    let Some(tri) = render(
        &mut pattern_sphere(TextureMapping::Triplanar, 0.8),
        "triplanar_sphere",
    ) else {
        return;
    };
    let (uv_seams, tri_seams) = (seam_pixels(&uv), seam_pixels(&tri));
    eprintln!("seam pixels: {uv_seams} with UV mapping, {tri_seams} triplanar");
    assert!(
        uv_seams >= 15,
        "the control should show its UV seam ({uv_seams} seam pixels)"
    );
    assert!(
        tri_seams <= 3,
        "triplanar mapping shows {tri_seams} seam pixels (UV control: {uv_seams})"
    );
}

#[test]
fn a_preset_draws_through_the_mesh_pipeline() {
    let mut app = headless_app(160, 160, |_| {});
    app.world.spawn((
        Camera {
            exposure: Exposure::Manual { ev100: 14.0 },
            ..Camera::perspective(50.0)
        },
        Transform::from_xyz(0.0, 0.0, 2.0).looking_at(DVec3::ZERO, Vec3::Y),
    ));
    app.world.spawn((
        DirectionalLight::default(),
        Transform::default().looking_at(DVec3::new(0.3, -0.4, -1.0), Vec3::Y),
    ));
    app.world.spawn((
        Transform::default(),
        MeshRenderer::new(
            Shape::Quad { size: [1.6, 1.6] },
            StandardMaterial::preset(MaterialPreset::Brick).with_texture_scale(0.8),
        ),
    ));
    let Some(img) = render(&mut app, "preset_brick_quad") else {
        return;
    };
    // Bricks and mortar: strong contrast in the middle, and red dominates.
    let (mut sum, mut sum2, mut red, mut n) = (0.0, 0.0, 0.0, 0.0);
    for y in 40..120 {
        for x in 40..120 {
            let p = img.get_pixel(x, y);
            let l = luma(p);
            sum += l;
            sum2 += l * l;
            red += p[0] as f32 - p[2] as f32;
            n += 1.0;
        }
    }
    let mean = sum / n;
    let spread = (sum2 / n - mean * mean).max(0.0).sqrt();
    assert!(mean > 20.0, "the wall is lit (mean luma {mean:.1})");
    assert!(
        spread > 12.0,
        "bricks and joints differ (spread {spread:.1})"
    );
    assert!(red / n > 15.0, "brick is red (red - blue {:.1})", red / n);
}
