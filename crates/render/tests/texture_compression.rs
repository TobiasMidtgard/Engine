//! Texture compression (`RenderSettings::texture_compression`): once a material image has been
//! drawn unchanged for `COMPRESS_AFTER_FRAMES`, the renderer block-compresses it on the GPU and
//! swaps it in. The picture stays close to the RGBA8 one, color goes BC7 and one-channel slots
//! BC4, and images that can't be compressed stay as they were. Needs a GPU or a software Vulkan
//! driver (lavapipe); passes with a note when neither is available.

use std::path::Path;
use std::time::Instant;

use engine_assets::{AssetPlugin, Assets, Handle};
use engine_core::prelude::*;
use engine_render::image_cache::{COMPRESS_AFTER_FRAMES, ImageMemoryStats};
use engine_render::prelude::*;
use engine_render::{Exposure, RenderDisabled, Renderer, TextureUse, capture_now, wgpu};
use image::RgbaImage;

const SIZE: u32 = 192;

/// A still headless scene (no jitter, grain or temporal effects) looking at the origin from +Z.
fn still_app(compression: TextureCompression) -> App {
    engine_core::log::init("error");
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = SIZE;
        w.height = SIZE;
    }
    app.add_plugins(AssetPlugin {
        hot_reload: false,
        root: None,
    });
    app.add_plugins(RenderPlugin);
    {
        let mut s = app.world.resource_mut::<RenderSettings>();
        s.texture_compression = compression;
        s.texture_compression_on_cpu = true;
        s.anti_aliasing = AntiAliasing::None;
        s.film_grain = 0.0;
        s.sky_cloud_layer = false;
        s.volumetric_fog = false;
        s.screen_space_reflections = false;
        s.screen_space_gi = false;
        s.ambient_occlusion = false;
        s.contact_shadow_length = 0.0;
        s.shadow_map_size = 512;
        s.far_shadow_distance = 0.0;
    }
    app.world.spawn((
        Camera {
            exposure: Exposure::Manual { ev100: 14.0 },
            ..Camera::perspective(60.0)
        },
        Transform::from_xyz(0.0, 0.0, 2.2).looking_at(DVec3::ZERO, Vec3::Y),
    ));
    app.world.spawn((
        DirectionalLight::default(),
        Transform::default().looking_at(DVec3::new(0.3, -0.3, -1.0), Vec3::Y),
    ));
    app
}

/// A photo-like texture: smooth gradients (where BC1 bands and BC7 doesn't) with fine detail.
fn photo(size: u32) -> Image {
    let mut image = Image::from_fn(size, size, true, |x, y| {
        let (u, v) = (x as f32 / size as f32, y as f32 / size as f32);
        let detail = ((x * 7 + y * 13) % 17) as f32 / 17.0 * 24.0;
        [
            (40.0 + 180.0 * u + detail) as u8,
            (60.0 + 140.0 * v) as u8,
            (200.0 - 120.0 * u * v + detail * 0.5) as u8,
            255,
        ]
    });
    image.generate_mips();
    image
}

fn add(app: &mut App, image: Image) -> Handle<Image> {
    app.world.resource_mut::<Assets<Image>>().add(image)
}

/// Run `frames` frames; false (with a note) when there is no GPU.
fn run(app: &mut App, frames: usize, test: &str) -> bool {
    app.update_n(frames);
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping {test}: no GPU ({})", d.0);
        return false;
    }
    true
}

fn bc_supported(app: &App) -> bool {
    let shared = &app.world.resource::<Renderer>().shared;
    let ok = shared
        .features
        .contains(wgpu::Features::TEXTURE_COMPRESSION_BC)
        && shared.adapter_info.backend != wgpu::Backend::Gl;
    if !ok {
        eprintln!("skipping: the GPU has no BC formats");
    }
    ok
}

fn cache(app: &App) -> ImageMemoryStats {
    app.world.resource::<Renderer>().shared.images.stats()
}

/// Render a frame and read it back. Panics on GPU validation errors.
fn shot(app: &mut App, name: &str) -> RgbaImage {
    let path =
        Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("texture_compression_{name}.png"));
    capture_now(&mut app.world, &path).expect("screenshot");
    if let Ok(out) = std::env::var("ENGINE_SHOTS") {
        let _ = std::fs::copy(&path, Path::new(&out).join(format!("{name}.png")));
    }
    let gpu_errors: Vec<String> = engine_core::log::log_buffer()
        .tail(64, Some("error"))
        .into_iter()
        .filter(|l| l.message.contains("GPU error"))
        .map(|l| l.message)
        .collect();
    assert!(
        gpu_errors.is_empty(),
        "GPU errors in {name}: {gpu_errors:#?}"
    );
    image::open(&path).expect("png").to_rgba8()
}

/// Mean and largest per-channel difference between two shots.
fn difference(a: &RgbaImage, b: &RgbaImage) -> (f32, u8) {
    let (mut sum, mut max) = (0u64, 0u8);
    for (p, q) in a.pixels().zip(b.pixels()) {
        for k in 0..3 {
            let d = p.0[k].abs_diff(q.0[k]);
            sum += d as u64;
            max = max.max(d);
        }
    }
    (sum as f32 / (a.width() * a.height() * 3) as f32, max)
}

/// The scene: a textured quad with a color map and an occlusion map, drawn until compressed.
fn scene(compression: TextureCompression, name: &str) -> Option<(App, RgbaImage)> {
    let mut app = still_app(compression);
    let color = add(&mut app, photo(256));
    let mut ao = Image::from_fn(128, 128, false, |x, y| {
        let g = (128 + (x as i32 - 64) + (y as i32 - 64) / 2).clamp(0, 255) as u8;
        [g, g, g, 255]
    });
    ao.generate_mips();
    let ao = add(&mut app, ao);
    let mut material = StandardMaterial::color(1.0, 1.0, 1.0)
        .with_roughness(0.9)
        .with_base_color_texture(color);
    material.occlusion_texture = Some(ao);
    app.world.spawn((
        Transform::default(),
        MeshRenderer::new(Shape::Quad { size: [2.0, 2.0] }, material),
    ));
    if !run(&mut app, 4, name) || !bc_supported(&app) {
        return None;
    }
    assert_eq!(cache(&app).compressed, 0, "compressed before the delay");
    // The encoders build on a thread (slowly under a software GPU): draw until both slots are
    // compressed, or for as many frames as the delay when nothing is to be compressed.
    let started = Instant::now();
    app.update_n(COMPRESS_AFTER_FRAMES as usize + 2);
    let mut frames = COMPRESS_AFTER_FRAMES + 2;
    while compression != TextureCompression::Off && cache(&app).compressed < 2 && frames < 2000 {
        app.update();
        frames += 1;
    }
    eprintln!(
        "{name}: {frames} frames in {:.2} s",
        started.elapsed().as_secs_f32()
    );
    let shot = shot(&mut app, name);
    Some((app, shot))
}

#[test]
fn material_images_compress_and_look_the_same() {
    let Some((_, plain)) = scene(TextureCompression::Off, "plain") else {
        return;
    };
    let Some((app, compressed)) = scene(TextureCompression::Quality, "bc7") else {
        return;
    };
    let stats = cache(&app);
    eprintln!("compressed: {stats:?}");
    // Color (BC7, 256² with mips) and occlusion (BC4, 128² with mips).
    assert_eq!(stats.compressed, 2, "both slots compressed: {stats:?}");
    let rgba = |s: u64| (1..=8).map(|l| (s >> (l - 1)).pow(2)).sum::<u64>() * 4;
    let saved = rgba(256) * 3 / 4 + rgba(128) * 7 / 8;
    assert!(
        stats.bytes_compressed_away.abs_diff(saved) < saved / 50,
        "saved {} bytes, expected about {saved}",
        stats.bytes_compressed_away
    );
    let renderer = app.world.resource::<Renderer>();
    let images = app.world.resource::<Assets<Image>>();
    let handles: Vec<_> = images.iter().map(|(h, _)| h).collect();
    let formats: Vec<_> = handles
        .iter()
        .filter_map(|&h| {
            let color = renderer
                .shared
                .image_texture_for(images, h, TextureUse::Color)?;
            let red = renderer
                .shared
                .image_texture_for(images, h, TextureUse::Red)?;
            Some((color.format(), red.format()))
        })
        .collect();
    assert!(
        formats
            .iter()
            .any(|f| f.0 == wgpu::TextureFormat::Bc7RgbaUnormSrgb),
        "the color map is BC7 sRGB: {formats:?}"
    );
    assert!(
        formats
            .iter()
            .any(|f| f.1 == wgpu::TextureFormat::Bc4RUnorm),
        "the occlusion map is BC4: {formats:?}"
    );

    let (mean, max) = difference(&plain, &compressed);
    eprintln!("BC7 vs RGBA8: mean {mean:.3}, max {max}");
    assert!(mean < 1.5, "the picture changed: mean {mean}");
    assert!(max < 24, "a pixel changed a lot: {max}");
}

#[test]
fn images_that_dont_split_into_blocks_and_exact_slots_stay_rgba8() {
    let mut app = still_app(TextureCompression::Quality);
    // 100 → 50 → 25: the third level isn't whole 4×4 blocks.
    let mut odd = Image::from_fn(100, 100, true, |x, _| [x as u8, 90, 40, 255]);
    odd.generate_mips();
    let odd = add(&mut app, odd);
    let lut = add(&mut app, photo(64));
    app.world.spawn((
        Transform::default(),
        MeshRenderer::new(
            Shape::Quad { size: [2.0, 2.0] },
            StandardMaterial::color(1.0, 1.0, 1.0).with_base_color_texture(odd),
        ),
    ));
    if !run(&mut app, 2, "stay_rgba8") || !bc_supported(&app) {
        return;
    }
    {
        let renderer = app.world.resource::<Renderer>();
        let images = app.world.resource::<Assets<Image>>();
        renderer
            .shared
            .image_texture_for(images, lut, TextureUse::Exact)
            .expect("uploaded");
    }
    app.update_n(COMPRESS_AFTER_FRAMES as usize + 4);
    let renderer = app.world.resource::<Renderer>();
    let images = app.world.resource::<Assets<Image>>();
    // Keep asking, as a feature drawing it would.
    let exact = renderer
        .shared
        .image_texture_for(images, lut, TextureUse::Exact)
        .expect("uploaded");
    let odd = renderer
        .shared
        .image_texture_for(images, odd, TextureUse::Color)
        .expect("uploaded");
    assert_eq!(
        exact.format(),
        wgpu::TextureFormat::Rgba8UnormSrgb.remove_srgb_suffix()
    );
    assert_eq!(odd.format(), wgpu::TextureFormat::Rgba8UnormSrgb);
    assert_eq!(cache(&app).compressed, 0);
}

#[test]
fn compact_mode_uses_bc1_for_opaque_color_and_two_channel_slots_get_bc5() {
    let mut app = still_app(TextureCompression::Compact);
    let opaque = add(&mut app, photo(64));
    let mut cutout = photo(64);
    cutout
        .data
        .as_chunks_mut::<4>()
        .0
        .iter_mut()
        .step_by(3)
        .for_each(|p| p[3] = 0);
    cutout.generate_mips();
    let cutout = add(&mut app, cutout);
    let mut normals = Image::from_fn(64, 64, false, |x, y| [x as u8 * 4, y as u8 * 4, 255, 255]);
    normals.generate_mips();
    let normals = add(&mut app, normals);
    if !run(&mut app, 1, "compact") || !bc_supported(&app) {
        return;
    }
    let ask = |app: &App, h, usage| {
        let images = app.world.resource::<Assets<Image>>();
        app.world
            .resource::<Renderer>()
            .shared
            .image_texture_for(images, h, usage)
            .expect("uploaded")
    };
    let first = ask(&app, opaque, TextureUse::Color);
    ask(&app, cutout, TextureUse::Color);
    ask(&app, normals, TextureUse::RedGreen);
    app.update_n(COMPRESS_AFTER_FRAMES as usize + 2);
    let mut frames = 0;
    while cache(&app).compressed < 3 && frames < 2000 {
        app.update();
        frames += 1;
    }
    let (opaque, cutout, normals) = (
        ask(&app, opaque, TextureUse::Color),
        ask(&app, cutout, TextureUse::Color),
        ask(&app, normals, TextureUse::RedGreen),
    );
    assert_eq!(opaque.format(), wgpu::TextureFormat::Bc1RgbaUnormSrgb);
    assert_eq!(cutout.format(), wgpu::TextureFormat::Bc7RgbaUnormSrgb);
    assert_eq!(normals.format(), wgpu::TextureFormat::Bc5RgUnorm);
    // A feature sees the swap as a new id and rebuilds its bind groups.
    assert_ne!(first.id, opaque.id);
    assert_eq!(
        (opaque.width, opaque.height, opaque.mip_levels),
        (first.width, first.height, first.mip_levels)
    );
    let _ = shot(&mut app, "compact");
}
