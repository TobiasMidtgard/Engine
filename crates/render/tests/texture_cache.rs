//! The disk cache of compressed images (`block_cache`): the first run compresses a material
//! image on the GPU and stores its blocks; the next run uploads the same pixels compressed from
//! its first frame, with no RGBA8 copy on the GPU, and draws the same picture. Needs a GPU or a
//! software Vulkan driver (lavapipe); passes with a note when neither is available.

use std::path::{Path, PathBuf};

use engine_assets::{AssetPlugin, Assets, Handle};
use engine_core::prelude::*;
use engine_render::image_cache::{COMPRESS_AFTER_FRAMES, ImageMemoryStats};
use engine_render::prelude::*;
use engine_render::{Exposure, RenderDisabled, Renderer, TextureUse, capture_now, wgpu};
use image::RgbaImage;

const SIZE: u32 = 192;

/// A still headless scene (no jitter, grain or temporal effects) looking at the origin from +Z,
/// keeping its compressed images in `dir`. `None` (with a note) without a GPU with BC formats.
fn still_app(dir: &Path, test: &str) -> Option<App> {
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
        // The final image's dither seeds on real time (`post.wgsl`), so pictures compared
        // across runs take real time in whole seconds: the same dither however many frames
        // each run needed.
        let mut time = app.world.resource_mut::<Time>();
        time.set_max_delta(1.0);
        time.set_source(engine_core::time::TimeSource::Manual { step_secs: 1.0 });
    }
    {
        let mut s = app.world.resource_mut::<RenderSettings>();
        s.texture_compression = TextureCompression::Quality;
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
    // The renderer starts with the first frame.
    app.update();
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping {test}: no GPU ({})", d.0);
        return None;
    }
    let shared = &app.world.resource::<Renderer>().shared;
    if !shared
        .features
        .contains(wgpu::Features::TEXTURE_COMPRESSION_BC)
        || shared.adapter_info.backend == wgpu::Backend::Gl
    {
        eprintln!("skipping {test}: the GPU has no BC formats");
        return None;
    }
    shared.images.use_disk_cache(Some(dir.to_path_buf()));
    Some(app)
}

/// A photo-like texture: smooth gradients with fine detail; `seed` moves the detail.
fn photo(size: u32, seed: u32) -> Image {
    let mut image = Image::from_fn(size, size, true, |x, y| {
        let (u, v) = (x as f32 / size as f32, y as f32 / size as f32);
        let detail = ((x * 7 + y * 13 + seed) % 17) as f32 / 17.0 * 24.0;
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

fn stats(app: &App) -> ImageMemoryStats {
    app.world.resource::<Renderer>().shared.images.stats()
}

fn format(app: &App, handle: Handle<Image>, usage: TextureUse) -> wgpu::TextureFormat {
    let images = app.world.resource::<Assets<Image>>();
    app.world
        .resource::<Renderer>()
        .shared
        .image_texture_for(images, handle, usage)
        .expect("uploaded")
        .format()
}

/// A quad with a color map and an occlusion map.
fn spawn_quad(app: &mut App, seed: u32) -> (Handle<Image>, Handle<Image>) {
    let color = app
        .world
        .resource_mut::<Assets<Image>>()
        .add(photo(256, seed));
    let mut ao = Image::from_fn(128, 128, false, |x, y| {
        let g = (128 + (x as i32 - 64) + (y as i32 - 64) / 2).clamp(0, 255) as u8;
        [g, g, g, 255]
    });
    ao.generate_mips();
    let ao = app.world.resource_mut::<Assets<Image>>().add(ao);
    let mut material = StandardMaterial::color(1.0, 1.0, 1.0)
        .with_roughness(0.9)
        .with_base_color_texture(color);
    material.occlusion_texture = Some(ao);
    app.world.spawn((
        Transform::default(),
        MeshRenderer::new(Shape::Quad { size: [2.0, 2.0] }, material),
    ));
    (color, ao)
}

/// Render a frame and read it back. Panics on GPU validation errors.
fn shot(app: &mut App, name: &str) -> RgbaImage {
    let path = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("texture_cache_{name}.png"));
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

fn files(dir: &Path) -> usize {
    std::fs::read_dir(dir).map_or(0, |d| {
        d.flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "txc"))
            .count()
    })
}

fn temp_dir(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("texture_cache_{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

#[test]
fn the_second_run_uploads_images_compressed_and_looks_the_same() {
    let dir = temp_dir("second_run");

    // First run: nothing on disk, so the images go up as RGBA8 and are compressed on the GPU.
    let Some(mut first) = still_app(&dir, "second_run") else {
        return;
    };
    let (color, ao) = spawn_quad(&mut first, 0);
    first.update_n(3);
    assert_eq!(stats(&first).cache_hits, 0);
    assert_eq!(
        format(&first, color, TextureUse::Color),
        wgpu::TextureFormat::Rgba8UnormSrgb,
        "nothing cached yet: RGBA8 until compressed"
    );
    first.update_n(COMPRESS_AFTER_FRAMES as usize);
    let mut frames = 0;
    // Until both are compressed, read back and written.
    while (stats(&first).cache_stores < 2 || files(&dir) < 2) && frames < 3000 {
        first.update();
        frames += 1;
        if stats(&first).cache_stores >= 2 {
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }
    let s = stats(&first);
    assert_eq!((s.compressed, s.cache_stores), (2, 2), "{s:?}");
    assert_eq!(files(&dir), 2, "one file per compressed view");
    assert_eq!(
        format(&first, color, TextureUse::Color),
        wgpu::TextureFormat::Bc7RgbaUnormSrgb
    );
    assert_eq!(
        format(&first, ao, TextureUse::Red),
        wgpu::TextureFormat::Bc4RUnorm
    );
    // A drawn material binds the compressed texture at its next periodic check (at most 64
    // frames on, `mesh_feature`).
    first.update_n(66);
    let compressed = shot(&mut first, "first_run");
    drop(first);

    // Second run, same pixels: compressed from the first frame, nothing left to compress.
    let Some(mut second) = still_app(&dir, "second_run") else {
        return;
    };
    let (color, ao) = spawn_quad(&mut second, 0);
    second.update_n(2);
    let s = stats(&second);
    assert_eq!((s.cache_hits, s.compressed), (2, 0), "{s:?}");
    assert_eq!(
        format(&second, color, TextureUse::Color),
        wgpu::TextureFormat::Bc7RgbaUnormSrgb,
        "compressed from the first frame"
    );
    assert_eq!(
        format(&second, ao, TextureUse::Red),
        wgpu::TextureFormat::Bc4RUnorm
    );
    // What the RGBA8 copies would have taken on the GPU: three quarters of the color map
    // (BC7), seven eighths of the occlusion map (BC4).
    let rgba = |side: u64| (1..=9).map(|l| (side >> (l - 1)).pow(2)).sum::<u64>() * 4;
    let saved = rgba(256) * 3 / 4 + rgba(128) * 7 / 8;
    assert!(
        s.cache_bytes_saved.abs_diff(saved) < saved / 50,
        "saved {} bytes, expected about {saved}",
        s.cache_bytes_saved
    );
    // Nothing is compressed or stored again.
    second.update_n(COMPRESS_AFTER_FRAMES as usize + 4);
    let s = stats(&second);
    assert_eq!((s.compressed, s.cache_stores), (0, 0), "{s:?}");
    assert_eq!(files(&dir), 2);
    let cached = shot(&mut second, "second_run");

    // The very same blocks, so the very same picture.
    let (mean, worst) = difference(&compressed, &cached);
    eprintln!("cached vs compressed in place: mean {mean:.4}, worst {worst}");
    assert!(mean < 0.05, "the cached picture differs: mean {mean}");
    assert!(
        worst <= 2,
        "a pixel of the cached picture differs by {worst}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn other_pixels_miss_the_cache_and_a_damaged_file_is_made_again() {
    let dir = temp_dir("miss");
    let Some(mut first) = still_app(&dir, "miss") else {
        return;
    };
    spawn_quad(&mut first, 0);
    first.update_n(COMPRESS_AFTER_FRAMES as usize + 2);
    let mut frames = 0;
    while files(&dir) < 2 && frames < 3000 {
        first.update();
        frames += 1;
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert_eq!(files(&dir), 2);
    drop(first);

    // Damage every file: wrong blocks must never be drawn.
    for entry in std::fs::read_dir(&dir).unwrap().flatten() {
        let mut bytes = std::fs::read(entry.path()).unwrap();
        let at = bytes.len() / 2;
        bytes[at] ^= 0x55;
        std::fs::write(entry.path(), bytes).unwrap();
    }
    let Some(mut second) = still_app(&dir, "miss") else {
        return;
    };
    // Another color map (new pixels) beside the same occlusion map (damaged on disk).
    let (color, _) = spawn_quad(&mut second, 5);
    second.update_n(2);
    assert_eq!(stats(&second).cache_hits, 0, "{:?}", stats(&second));
    assert_eq!(
        format(&second, color, TextureUse::Color),
        wgpu::TextureFormat::Rgba8UnormSrgb
    );
    second.update_n(COMPRESS_AFTER_FRAMES as usize);
    let mut frames = 0;
    while stats(&second).cache_stores < 2 && frames < 3000 {
        second.update();
        frames += 1;
    }
    let s = stats(&second);
    assert_eq!((s.compressed, s.cache_stores), (2, 2), "{s:?}");
    let _ = shot(&mut second, "after_damage");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_image_that_is_redrawn_stays_out_of_the_cache() {
    let dir = temp_dir("redrawn");
    let Some(mut app) = still_app(&dir, "redrawn") else {
        return;
    };
    let (color, _) = spawn_quad(&mut app, 0);
    app.update_n(3);
    // New pixels after the first upload, as a map painted at run time gets.
    app.world
        .resource_mut::<Assets<Image>>()
        .get_mut(color)
        .expect("the color map")
        .data[..4]
        .copy_from_slice(&[255, 0, 255, 255]);
    app.update_n(COMPRESS_AFTER_FRAMES as usize + 2);
    let mut frames = 0;
    while (stats(&app).compressed < 2 || stats(&app).cache_stores < 1) && frames < 3000 {
        app.update();
        frames += 1;
    }
    // Long enough for a second readback to have come back, had there been one.
    app.update_n(8);
    let s = stats(&app);
    assert_eq!(s.compressed, 2, "both are still compressed: {s:?}");
    assert_eq!(s.cache_stores, 1, "only the occlusion map is stored: {s:?}");
    assert_eq!(
        format(&app, color, TextureUse::Color),
        wgpu::TextureFormat::Bc7RgbaUnormSrgb
    );
    let _ = std::fs::remove_dir_all(&dir);
}
