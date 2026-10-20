//! Releasing image pixels (`RenderSettings::release_image_pixels`): once the renderer has
//! uploaded an image and the image has stayed unchanged for `RELEASE_AFTER_FRAMES`, its CPU
//! pixels are freed and the GPU copy draws on. Needs a GPU or a software Vulkan driver
//! (lavapipe); passes with a note when neither is available.

use std::path::Path;
use std::sync::Arc;

use engine_assets::{AssetPlugin, Assets, Handle};
use engine_core::prelude::*;
use engine_render::image_cache::{ImageMemoryStats, RELEASE_AFTER_FRAMES};
use engine_render::prelude::*;
use engine_render::{
    Exposure, ImageTexture, PixelFormat, RenderDisabled, RenderStats, Renderer, capture_now,
};
use image::RgbaImage;

const SIZE: u32 = 96;
const RELEASE: usize = RELEASE_AFTER_FRAMES as usize;

/// Headless app looking at the origin from +Z, with a sun and nothing that changes from frame to
/// frame (no TAA jitter, film grain, moving clouds, temporal effects or auto-exposure), so two
/// frames of the same scene differ only by the tonemapper's dither (one 8-bit level each).
fn still_app(release: bool) -> App {
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
        s.release_image_pixels = release;
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
        Transform::from_xyz(0.0, 0.0, 3.0).looking_at(DVec3::ZERO, Vec3::Y),
    ));
    app.world.spawn((
        DirectionalLight::default(),
        Transform::default().looking_at(DVec3::new(0.3, -0.3, -1.0), Vec3::Y),
    ));
    app
}

/// A quad facing the camera at `x`, textured with `image`.
fn textured_quad(app: &mut App, image: Handle<Image>, x: f64, size: f32) {
    let material = StandardMaterial::color(1.0, 1.0, 1.0)
        .with_roughness(0.9)
        .with_base_color_texture(image);
    app.world.spawn((
        Transform::from_xyz(x, 0.0, 0.0),
        MeshRenderer::new(Shape::Quad { size: [size, size] }, material),
    ));
}

fn add(app: &mut App, image: Image) -> Handle<Image> {
    app.world.resource_mut::<Assets<Image>>().add(image)
}

fn solid(size: u32, rgba: [u8; 4]) -> Image {
    Image::from_fn(size, size, true, |_, _| rgba)
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

/// Render a frame and read it back. Panics on GPU validation errors.
fn shot(app: &mut App, name: &str) -> RgbaImage {
    let path = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("image_memory_{name}.png"));
    capture_now(&mut app.world, &path).expect("screenshot");
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

fn released(app: &App, h: Handle<Image>) -> bool {
    app.world
        .resource::<Assets<Image>>()
        .get(h)
        .expect("image")
        .is_released()
}

fn cache(app: &App) -> ImageMemoryStats {
    app.world.resource::<Renderer>().shared.images.stats()
}

/// The renderer's texture of `h` through the API features use.
fn shared_texture(app: &App, h: Handle<Image>, color: bool) -> Option<Arc<ImageTexture>> {
    let images = app.world.resource::<Assets<Image>>();
    app.world
        .resource::<Renderer>()
        .shared
        .image_texture(images, h, color)
}

fn diagnostic(app: &App, key: &str) -> f64 {
    app.world
        .resource::<Diagnostics>()
        .get(key)
        .unwrap_or_else(|| panic!("no diagnostic {key}"))
}

/// Mean color of the 5×5 pixels around the image center.
fn center_rgb(img: &RgbaImage) -> [f32; 3] {
    let c = SIZE / 2;
    let mut sum = [0.0f32; 3];
    for y in c - 2..=c + 2 {
        for x in c - 2..=c + 2 {
            let p = img.get_pixel(x, y);
            for (s, v) in sum.iter_mut().zip(p.0) {
                *s += v as f32;
            }
        }
    }
    sum.map(|s| s / 25.0)
}

#[test]
fn pixels_are_freed_after_the_delay_and_the_picture_stays_the_same() {
    let mut app = still_app(true);
    let mut checker = Image::checker(64, 4, [230, 60, 40, 255], [30, 60, 200, 255]);
    checker.generate_mips();
    let h = add(&mut app, checker);
    textured_quad(&mut app, h, 0.0, 2.0);
    if !run(&mut app, 4, "pixels_are_freed_after_the_delay") {
        return;
    }
    let before = shot(&mut app, "before_release");
    assert!(!released(&app, h), "released before the delay");
    assert_eq!(cache(&app).uploads, 1);
    // The texture the mesh feature drew with is the one other features get.
    let texture = shared_texture(&app, h, true).expect("uploaded");
    assert!(texture.srgb);
    assert_eq!(
        (texture.width, texture.height, texture.mip_levels),
        (64, 64, 7)
    );
    assert_eq!(cache(&app).uploads, 1);

    // Half the delay: still there.
    app.update_n(RELEASE / 2);
    assert!(!released(&app, h), "released after half the delay");

    app.update_n(RELEASE / 2 + 5);
    assert!(released(&app, h), "not released after the delay");
    {
        let images = app.world.resource::<Assets<Image>>();
        let image = images.get(h).unwrap();
        assert!(image.data.is_empty() && image.mips.is_empty());
        assert_eq!((image.width, image.height), (64, 64));
        assert_eq!(image.format, PixelFormat::Rgba8);
        assert!(image.srgb && image.mipmaps && !image.keep_pixels);
        // Code that reads pixels copes with a released image.
        assert_eq!(image.pixel(3, 3), [0; 4]);
        assert!(image.mip_chain(true).is_empty());
        assert!(!image.is_valid());
    }

    let after = shot(&mut app, "after_release");
    assert_eq!(
        cache(&app).uploads,
        1,
        "releasing the pixels uploaded the image again"
    );
    let same = shared_texture(&app, h, true).expect("still on the GPU");
    assert_eq!(
        same.id, texture.id,
        "releasing the pixels replaced the texture"
    );
    // Asked for as linear data now, the released image is the same texture seen linearly.
    let linear = shared_texture(&app, h, false).expect("a linear view of the GPU copy");
    assert!(!linear.srgb);
    assert_eq!((linear.width, linear.mip_levels), (64, 7));
    assert_eq!(cache(&app).uploads, 1);
    assert_eq!(cache(&app).released_now, 1);
    // The tonemapper dithers every frame by up to one 8-bit level, so two frames of the same
    // still scene differ by at most 2 per channel; anything else changed the picture.
    let worst = before
        .pixels()
        .zip(after.pixels())
        .map(|(a, b)| (0..3).map(|c| a[c].abs_diff(b[c])).max().unwrap_or(0))
        .max()
        .unwrap_or(0);
    assert!(
        worst <= 2,
        "the picture changed when the pixels were released (by up to {worst} levels)"
    );
    // The quad shows the checker (red and blue cells), not a white stand-in.
    let [r, _, b] = center_rgb(&after);
    let (x, y) = (SIZE / 2 - SIZE / 8, SIZE / 2 - SIZE / 8);
    let corner = after.get_pixel(x, y);
    assert!(
        (r - b).abs() > 40.0 || (corner[0] as f32 - corner[2] as f32).abs() > 40.0,
        "the quad lost its texture: center {:?}, near corner {corner:?}",
        center_rgb(&after)
    );
}

#[test]
fn new_pixels_after_release_show_the_new_color() {
    let mut app = still_app(true);
    let h = add(&mut app, solid(16, [220, 30, 30, 255]));
    textured_quad(&mut app, h, 0.0, 2.0);
    if !run(&mut app, RELEASE + 5, "new_pixels_after_release") {
        return;
    }
    assert!(released(&app, h), "not released after the delay");
    let red = center_rgb(&shot(&mut app, "red_released"));
    assert!(red[0] > red[1] + 40.0 && red[0] > red[2] + 40.0, "{red:?}");

    // Assign new pixels: green.
    {
        let mut images = app.world.resource_mut::<Assets<Image>>();
        let image = images.get_mut(h).unwrap();
        image.data = [30u8, 200, 40, 255].repeat(16 * 16);
    }
    app.update_n(2);
    assert!(!released(&app, h));
    let green = center_rgb(&shot(&mut app, "green_after_release"));
    assert!(
        green[1] > green[0] + 40.0 && green[1] > green[2] + 40.0,
        "new pixels don't show: {green:?} (was {red:?})"
    );
    assert_eq!(cache(&app).uploads, 2);

    // And they are released again once they have stayed for the delay.
    app.update_n(RELEASE + 5);
    assert!(released(&app, h));
    let still_green = center_rgb(&shot(&mut app, "green_released_again"));
    // Equal up to the tonemapper's per-frame dither.
    assert!(
        still_green
            .iter()
            .zip(green)
            .all(|(a, b)| (a - b).abs() <= 1.0),
        "{still_green:?} vs {green:?}"
    );
    assert_eq!(cache(&app).released, 2);
}

#[test]
fn nothing_is_freed_with_the_setting_off() {
    let mut app = still_app(false);
    let mut checker = Image::checker(32, 4, [200, 200, 200, 255], [20, 20, 20, 255]);
    checker.generate_mips();
    let bytes = checker.pixel_bytes();
    let h = add(&mut app, checker);
    textured_quad(&mut app, h, 0.0, 2.0);
    if !run(
        &mut app,
        2 * RELEASE + 5,
        "nothing_is_freed_with_the_setting_off",
    ) {
        return;
    }
    assert!(!released(&app, h));
    let images = app.world.resource::<Assets<Image>>();
    assert_eq!(images.get(h).unwrap().pixel_bytes(), bytes);
    assert!(images.get(h).unwrap().is_valid());
    let stats = cache(&app);
    assert_eq!((stats.gpu_images, stats.uploads), (1, 1), "{stats:?}");
    assert_eq!((stats.released, stats.bytes_released), (0, 0));
    assert_eq!(diagnostic(&app, "render.images_released"), 0.0);
    assert_eq!(diagnostic(&app, "render.image_bytes_released"), 0.0);
}

#[test]
fn images_that_keep_changing_are_never_freed() {
    let mut app = still_app(true);
    let every_frame = add(&mut app, solid(16, [200, 200, 200, 255]));
    let every_few = add(&mut app, solid(16, [200, 200, 200, 255]));
    let still = add(&mut app, solid(16, [200, 200, 200, 255]));
    textured_quad(&mut app, every_frame, -1.0, 0.9);
    textured_quad(&mut app, every_few, 0.0, 0.9);
    textured_quad(&mut app, still, 1.0, 0.9);
    if !run(&mut app, 1, "images_that_keep_changing_are_never_freed") {
        return;
    }
    for frame in 0..2 * RELEASE + 10 {
        {
            let mut images = app.world.resource_mut::<Assets<Image>>();
            // Written in place: this would panic on a released image.
            images.get_mut(every_frame).unwrap().data[0] = frame as u8;
            if frame % 5 == 0 {
                images.get_mut(every_few).unwrap().data[4] = frame as u8;
            }
        }
        app.update();
        assert!(!released(&app, every_frame), "frame {frame}");
        assert!(!released(&app, every_few), "frame {frame}");
    }
    // The setting is on: the image that stayed the same was released.
    assert!(released(&app, still));
    assert_eq!(cache(&app).released, 1);
}

#[test]
fn opted_out_images_keep_their_pixels() {
    let mut app = still_app(true);
    let mut kept = Image::checker(32, 4, [200, 120, 40, 255], [40, 40, 40, 255]);
    kept.keep_pixels = true;
    let original = kept.clone();
    let kept = add(&mut app, kept);
    let other = add(&mut app, solid(16, [200, 200, 200, 255]));
    textured_quad(&mut app, kept, -0.6, 1.0);
    textured_quad(&mut app, other, 0.6, 1.0);
    if !run(
        &mut app,
        2 * RELEASE + 5,
        "opted_out_images_keep_their_pixels",
    ) {
        return;
    }
    assert!(released(&app, other), "the setting is on");
    assert!(!released(&app, kept));
    let images = app.world.resource::<Assets<Image>>();
    assert!(images.get(kept).unwrap() == &original);
    assert_eq!(cache(&app).released_now, 1);
}

#[test]
fn stats_report_the_freed_bytes() {
    let mut app = still_app(true);
    // 64² RGBA8 with a stored mip chain: (4096 + 1024 + 256 + 64 + 16 + 4 + 1) × 4 bytes.
    let mut a = Image::checker(64, 8, [200, 200, 200, 255], [20, 20, 20, 255]);
    a.generate_mips();
    // 32² RGBA8, mips made on upload only: 4096 bytes.
    let b = solid(32, [90, 160, 220, 255]);
    let expected = (4096 + 1024 + 256 + 64 + 16 + 4 + 1) * 4 + 32 * 32 * 4;
    assert_eq!(a.pixel_bytes() + b.pixel_bytes(), expected);
    let a = add(&mut app, a);
    let b = add(&mut app, b);
    textured_quad(&mut app, a, -0.6, 1.0);
    textured_quad(&mut app, b, 0.6, 1.0);
    if !run(&mut app, 10, "stats_report_the_freed_bytes") {
        return;
    }
    assert_eq!(diagnostic(&app, "render.images_released"), 0.0);
    assert_eq!(diagnostic(&app, "render.image_bytes_released"), 0.0);

    app.update_n(RELEASE);
    assert!(released(&app, a) && released(&app, b));
    assert_eq!(diagnostic(&app, "render.images_released"), 2.0);
    assert_eq!(
        diagnostic(&app, "render.image_bytes_released"),
        expected as f64
    );
    assert_eq!(diagnostic(&app, "render.image_uploads"), 2.0);
    assert_eq!(diagnostic(&app, "render.gpu_images"), 2.0);
    let stats = app.world.resource::<RenderStats>();
    assert_eq!(stats.images_released, 2);
    assert_eq!(stats.image_bytes_released, expected as u64);
    let described = app.world.resource::<Renderer>().describe();
    assert_eq!(described["images"]["bytes_released"], expected as u64);
}
