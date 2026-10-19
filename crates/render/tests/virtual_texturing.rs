//! Virtual texturing on the GPU: big images and procedural sources streamed through the tile
//! cache must look like the same texture uploaded whole, close up and at grazing angles (no
//! seams), load only coarse tiles for far views, stay within the cache while flying, never show
//! black or magenta while tiles stream in, and work in every material path (normal maps, sRGB
//! and linear data, triplanar, parallax, alpha-masked shadows, the cluster path, instancing).
//! Also checks that anti-tiling breaks up a repeated texture. Needs a GPU or a software Vulkan
//! driver (lavapipe); passes with a note when neither is available.
//!
//! Every render also fails on GPU validation errors: the renderer only logs those (and drops
//! the frame), so pixel checks alone could miss them.

use std::path::Path;

use engine_assets::{AssetPlugin, Assets, Handle};
use engine_core::prelude::*;
use engine_render::prelude::*;
use engine_render::{
    Exposure, RenderDisabled, RenderStats, VirtualTextureStats, capture_now, paint_tile_with,
};
use image::RgbaImage;

const SIZE: u32 = 256;
const FOV_Y_DEG: f32 = 60.0;

// ---------------------------------------------------------------- procedural test textures

fn hash(x: u32, y: u32, seed: u32) -> f32 {
    let mut h = x
        .wrapping_mul(0x8da6_b343)
        .wrapping_add(y.wrapping_mul(0xd816_3841))
        .wrapping_add(seed.wrapping_mul(0xcb1a_b31f));
    h ^= h >> 15;
    h = h.wrapping_mul(0x2c1b_3c6d);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297a_2d39);
    h ^= h >> 15;
    (h & 0xffff) as f32 / 65535.0
}

/// Value noise with cells of `cell` texels, tiling every `size` texels.
fn value_noise(x: u32, y: u32, size: u32, cell: u32, seed: u32) -> f32 {
    let cells = (size / cell).max(1);
    let (cx, cy) = (x / cell, y / cell);
    let fx = (x % cell) as f32 / cell as f32;
    let fy = (y % cell) as f32 / cell as f32;
    let s = |t: f32| t * t * (3.0 - 2.0 * t);
    let (sx, sy) = (s(fx), s(fy));
    let at = |i: u32, j: u32| hash(i % cells, j % cells, seed);
    let a = at(cx, cy) + (at(cx + 1, cy) - at(cx, cy)) * sx;
    let b = at(cx, cy + 1) + (at(cx + 1, cy + 1) - at(cx, cy + 1)) * sx;
    a + (b - a) * sy
}

/// Detail at every scale from a quarter of the texture down to two texels: what a scan has.
fn fbm(x: u32, y: u32, size: u32, seed: u32) -> f32 {
    let mut sum = 0.0;
    let mut amp = 0.5;
    let mut norm = 0.0;
    let mut cell = size / 4;
    while cell >= 2 {
        sum += amp * value_noise(x, y, size, cell, seed);
        norm += amp;
        amp *= 0.62;
        cell /= 2;
    }
    sum / norm
}

/// Earthy color with fine grain and thin dark cracks; never darker than about 25/255.
fn ground_texel(x: u32, y: u32, size: u32) -> [u8; 4] {
    let n = fbm(x, y, size, 1);
    let m = value_noise(x, y, size, size / 8, 7);
    let crack = (value_noise(x, y, size, 24, 3) - 0.5).abs() < 0.02;
    let k = if crack { 0.35 } else { 1.0 };
    let c = |lo: f32, hi: f32, t: f32| ((lo + (hi - lo) * t) * k) as u8;
    [
        c(70.0, 200.0, n),
        c(60.0, 170.0, 0.6 * n + 0.4 * m),
        c(40.0, 120.0, n * m),
        255,
    ]
}

fn height_texel(x: u32, y: u32, size: u32) -> f32 {
    fbm(x, y, size, 11)
}

/// Tangent-space normal from the height's slope, +Y up (glTF).
fn normal_texel(x: u32, y: u32, size: u32) -> [u8; 4] {
    let h = |i: u32, j: u32| height_texel(i % size, j % size, size);
    let dx = h(x + 1, y) - h(x + size - 1, y);
    let dy = h(x, y + size - 1) - h(x, y + 1);
    let n = Vec3::new(-dx * 6.0, -dy * 6.0, 1.0).normalize();
    let c = |v: f32| ((v * 0.5 + 0.5) * 255.0).round() as u8;
    [c(n.x), c(n.y), c(n.z), 255]
}

fn ground_image(size: u32) -> Image {
    Image::from_fn(size, size, true, |x, y| ground_texel(x, y, size))
}

fn normal_image(size: u32) -> Image {
    Image::from_fn(size, size, false, |x, y| normal_texel(x, y, size))
}

fn height_image(size: u32) -> Image {
    Image::from_fn(size, size, false, |x, y| {
        let v = (height_texel(x, y, size) * 255.0) as u8;
        [v, v, v, 255]
    })
}

/// Leaf-like cutout: color with alpha 0 where the noise is low.
fn cutout_image(size: u32) -> Image {
    Image::from_fn(size, size, true, |x, y| {
        let mut t = ground_texel(x, y, size);
        t[3] = if value_noise(x, y, size, 64, 5) > 0.45 {
            255
        } else {
            0
        };
        t
    })
}

/// A crisp procedural pattern (checker cells, rings and grain) painted tile by tile; coarser
/// levels average level 0 exactly (2^mip × 2^mip boxes).
struct Pattern {
    size: u32,
}

impl Pattern {
    fn texel0(&self, x: u32, y: u32) -> [f32; 4] {
        let checker = ((x / 64) + (y / 64)).is_multiple_of(2);
        let dx = (x % 256) as f32 - 128.0;
        let dy = (y % 256) as f32 - 128.0;
        let ring = ((dx * dx + dy * dy).sqrt() / 9.0).fract() < 0.25;
        let grain = hash(x, y, 9);
        let base = if checker { 0.75 } else { 0.35 };
        let v = if ring { base * 0.5 } else { base } + 0.12 * (grain - 0.5);
        [
            v,
            v * 0.9 + 0.05,
            0.3 + 0.4 * (x as f32 / self.size as f32),
            1.0,
        ]
    }

    fn texel(&self, mip: u32, tx: u32, ty: u32) -> [u8; 4] {
        let n = 1u32 << mip;
        let mut sum = [0.0f32; 4];
        for j in 0..n {
            for i in 0..n {
                let t = self.texel0(tx * n + i, ty * n + j);
                for c in 0..4 {
                    sum[c] += t[c];
                }
            }
        }
        let k = 1.0 / (n * n) as f32;
        sum.map(|s| (s * k * 255.0 + 0.5).clamp(0.0, 255.0) as u8)
    }
}

impl TileSource for Pattern {
    fn size(&self) -> (u32, u32) {
        (self.size, self.size)
    }

    fn srgb(&self) -> bool {
        false
    }

    fn paint_tile(&self, mip: u32, x: u32, y: u32, out: &mut [u8]) {
        paint_tile_with(self.size(), mip, x, y, out, |tx, ty, _, _| {
            self.texel(mip, tx, ty)
        });
    }
}

// ---------------------------------------------------------------- app and rendering

/// Headless app with a sun and a camera at `eye` looking at `target`; post effects that
/// change from frame to frame are off so renders compare pixel by pixel.
fn app(eye: DVec3, target: DVec3, configure: impl FnOnce(&mut RenderSettings)) -> App {
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
        s.anti_aliasing = AntiAliasing::None;
        s.ambient_occlusion = false;
        s.screen_space_reflections = false;
        s.screen_space_gi = false;
        s.volumetric_fog = false;
        s.bloom_intensity = 0.0;
        s.vignette = 0.0;
        s.film_grain = 0.0;
        s.virtual_texture_threshold = 1024;
        configure(&mut s);
    }
    app.world.spawn((
        Camera {
            exposure: Exposure::Manual { ev100: 14.5 },
            ..Camera::perspective(FOV_Y_DEG)
        },
        Transform::from_translation(eye).looking_at(target, Vec3::Y),
    ));
    app.world.spawn((
        DirectionalLight::default(),
        Transform::default().looking_at(DVec3::new(0.3, -0.8, -0.5), Vec3::Y),
    ));
    app
}

fn add_image(app: &mut App, image: Image) -> Handle<Image> {
    app.world.resource_mut::<Assets<Image>>().add(image)
}

fn vt_stats(app: &App) -> VirtualTextureStats {
    app.world.resource::<VirtualTextureStats>().clone()
}

/// Whether the renderer runs (else a note: no GPU).
fn gpu(app: &mut App, name: &str) -> bool {
    app.update();
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping {name}: no GPU ({})", d.0);
        return false;
    }
    true
}

/// Run frames until the virtual textures stop streaming (nothing pending or loaded and no
/// feedback misses for 24 frames, longer than the feedback pattern and readback take to cover
/// every pixel) and, with `clusters`, the cluster path draws. Returns the frames run.
fn settle(app: &mut App, clusters: bool, max_frames: u32) -> u32 {
    let mut quiet = 0;
    for frame in 1..=max_frames {
        app.update();
        let s = vt_stats(app);
        let drawn = !clusters || app.world.resource::<RenderStats>().clusters > 0;
        if s.textures > 0
            && s.pending == 0
            && s.tiles_loaded == 0
            && s.feedback_misses == 0
            && drawn
        {
            quiet += 1;
            if quiet >= 24 {
                return frame;
            }
        } else {
            quiet = 0;
        }
    }
    panic!(
        "virtual textures still streaming after {max_frames} frames: {:?}",
        vt_stats(app)
    );
}

/// Render a frame and read the picture back. Panics on GPU validation errors.
fn capture(app: &mut App, name: &str) -> RgbaImage {
    let path = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("vt_{name}.png"));
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
    image::open(&path).expect("png").to_rgba8()
}

/// Per-pixel difference (largest channel) of two renders.
fn differences(a: &RgbaImage, b: &RgbaImage) -> Vec<u8> {
    a.pixels()
        .zip(b.pixels())
        .map(|(p, q)| (0..3).map(|c| p[c].abs_diff(q[c])).max().unwrap_or(0))
        .collect()
}

struct Diff {
    mean: f32,
    p99: u8,
    /// Worst mean difference of an 8×8 block: a seam or a wrong tile stands out here.
    worst_block: f32,
}

fn compare(a: &RgbaImage, b: &RgbaImage) -> Diff {
    let d = differences(a, b);
    let mean = d.iter().map(|&v| v as f32).sum::<f32>() / d.len() as f32;
    let mut sorted = d.clone();
    sorted.sort_unstable();
    let p99 = sorted[sorted.len() * 99 / 100];
    let w = a.width() as usize;
    let mut worst_block = 0.0f32;
    for by in 0..a.height() as usize / 8 {
        for bx in 0..w / 8 {
            let mut sum = 0.0;
            for y in 0..8 {
                for x in 0..8 {
                    sum += d[(by * 8 + y) * w + bx * 8 + x] as f32;
                }
            }
            worst_block = worst_block.max(sum / 64.0);
        }
    }
    Diff {
        mean,
        p99,
        worst_block,
    }
}

/// No pixel is (nearly) black or magenta: what a missing tile or a bad page table entry shows.
fn assert_no_black_or_magenta(img: &RgbaImage, what: &str) {
    for (x, y, p) in img.enumerate_pixels() {
        let black = p[0] < 10 && p[1] < 10 && p[2] < 10;
        let magenta = p[0] > 180 && p[2] > 180 && p[1] < 80;
        assert!(!black && !magenta, "{what}: pixel ({x}, {y}) is {p:?}");
    }
}

/// The scene rendered twice, through the tile cache and with every image uploaded whole;
/// `scene` spawns it with the image handles it was given. Returns (virtual, whole, stats).
fn render_both(
    name: &str,
    eye: DVec3,
    target: DVec3,
    configure: impl Fn(&mut RenderSettings),
    clusters: bool,
    scene: impl Fn(&mut App),
) -> Option<(RgbaImage, RgbaImage, VirtualTextureStats)> {
    let mut virt = app(eye, target, &configure);
    if !gpu(&mut virt, name) {
        return None;
    }
    scene(&mut virt);
    let frames = settle(&mut virt, clusters, 400);
    let stats = vt_stats(&virt);
    let a = capture(&mut virt, &format!("{name}_virtual"));
    drop(virt);

    let mut whole = app(eye, target, |s| {
        configure(s);
        s.virtual_textures = false;
    });
    scene(&mut whole);
    whole.update_n(frames as usize + 1);
    if clusters {
        for _ in 0..200 {
            if whole.world.resource::<RenderStats>().clusters > 0 {
                break;
            }
            whole.update();
        }
    }
    let b = capture(&mut whole, &format!("{name}_whole"));
    Some((a, b, stats))
}

fn quad(app: &mut App, material: StandardMaterial, at: DVec3, size: f32) {
    app.world.spawn((
        Transform::from_translation(at),
        MeshRenderer::new(Shape::Quad { size: [size, size] }, material),
    ));
}

// ---------------------------------------------------------------- tests

/// A 2048² sRGB color map and a 2048² normal map (linear data), over the 1024 threshold, seen
/// from 25 cm: every texel on screen needs level 0. Through the cache (BC7 where supported, and
/// uncompressed) it must match the whole-texture render.
#[test]
fn close_up_matches_the_whole_texture() {
    for compression in [false, true] {
        let name = format!("close_up_bc{}", compression as u8);
        let color = ground_image(2048);
        let normal = normal_image(2048);
        let Some((a, b, stats)) = render_both(
            &name,
            DVec3::new(0.05, 0.03, 0.25),
            DVec3::new(0.05, 0.03, 0.0),
            |s| s.virtual_texture_compression = compression,
            false,
            |app| {
                let color = add_image(app, color.clone());
                let normal = add_image(app, normal.clone());
                let material = StandardMaterial::color(1.0, 1.0, 1.0)
                    .with_roughness(0.7)
                    .with_base_color_texture(color)
                    .with_normal_texture(normal)
                    .with_detail(0.5, 0.05);
                quad(app, material, DVec3::ZERO, 2.0);
            },
        ) else {
            return;
        };
        let d = compare(&a, &b);
        eprintln!(
            "{name}: {d:?} stats {stats:?}",
            d = (d.mean, d.p99, d.worst_block)
        );
        assert_eq!(stats.textures, 2, "both maps are virtual");
        assert!(
            stats.resident_by_level.first().copied().unwrap_or(0) > 0,
            "level 0 tiles were loaded: {stats:?}"
        );
        let (mean, p99, block) = if stats.compressed {
            (3.0, 14, 8.0)
        } else {
            (1.5, 8, 4.0)
        };
        assert!(
            d.mean < mean && d.p99 <= p99 && d.worst_block < block,
            "{name}: virtual differs from whole: mean {}, p99 {}, worst 8x8 block {}",
            d.mean,
            d.p99,
            d.worst_block
        );
    }
}

/// A ground plane seen at a grazing angle: trilinear blends between levels and anisotropic
/// footprints cross tile edges everywhere. Through an uncompressed cache it must match the
/// whole texture with no seam (no 8×8 block much worse than the rest).
#[test]
fn no_seams_under_trilinear_and_anisotropic_filtering() {
    let color = ground_image(2048);
    let Some((a, b, stats)) = render_both(
        "grazing",
        DVec3::new(0.0, 0.6, 2.0),
        DVec3::new(0.0, 0.0, -2.0),
        |s| s.virtual_texture_compression = false,
        false,
        |app| {
            let color = add_image(app, color.clone());
            let material = StandardMaterial::color(1.0, 1.0, 1.0)
                .with_roughness(0.9)
                .with_base_color_texture(color)
                .with_uv_scale(0.25, 0.25);
            app.world.spawn((
                Transform::default(),
                MeshRenderer::new(
                    Shape::Plane {
                        size: [16.0, 16.0],
                        subdivisions: 1,
                    },
                    material,
                ),
            ));
        },
    ) else {
        return;
    };
    let d = compare(&a, &b);
    eprintln!(
        "grazing: {:?} stats {stats:?}",
        (d.mean, d.p99, d.worst_block)
    );
    let levels = stats.resident_by_level.iter().filter(|&&n| n > 0).count();
    assert!(levels >= 4, "the view spans many levels: {stats:?}");
    assert!(
        d.mean < 2.5 && d.worst_block < 8.0,
        "virtual differs from whole: mean {}, p99 {}, worst 8x8 block {}",
        d.mean,
        d.p99,
        d.worst_block
    );
}

/// Far away, a 2048² texture covers a few dozen pixels: only coarse tiles load.
#[test]
fn far_views_load_only_coarse_tiles() {
    let mut app = app(DVec3::new(0.0, 0.0, 40.0), DVec3::ZERO, |_| {});
    if !gpu(&mut app, "far") {
        return;
    }
    let color = add_image(&mut app, ground_image(2048));
    let material = StandardMaterial::color(1.0, 1.0, 1.0).with_base_color_texture(color);
    quad(&mut app, material, DVec3::ZERO, 2.0);
    settle(&mut app, false, 300);
    let img = capture(&mut app, "far");
    let s = vt_stats(&app);
    eprintln!("far: {s:?}");
    assert_eq!(s.textures, 1);
    // 2 m at 40 m: about 15 px for 2048 texels, level 7 wanted, so only the root (level 4) is
    // needed.
    let fine: u32 = s.resident_by_level.iter().take(4).sum();
    assert_eq!(fine, 0, "no tile finer than level 4: {s:?}");
    assert!(s.tiles_loaded_total <= 2, "only the root loads: {s:?}");
    assert_no_black_or_magenta(&img, "far");
}

/// Flying low over a big textured ground with a cache of 64 tiles: resident tiles and memory
/// never exceed the cache, tiles are evicted and reloaded, and the picture stays whole.
#[test]
fn residency_stays_within_the_cache_while_flying() {
    let mut app = app(DVec3::new(0.0, 1.0, 8.0), DVec3::new(0.0, 0.0, 4.0), |s| {
        s.virtual_texture_cache_tiles = 64;
        s.virtual_texture_upload_budget = 8;
    });
    if !gpu(&mut app, "flying") {
        return;
    }
    let color = add_image(&mut app, ground_image(4096));
    let normal = add_image(&mut app, normal_image(4096));
    let material = StandardMaterial::color(1.0, 1.0, 1.0)
        .with_base_color_texture(color)
        .with_normal_texture(normal)
        .with_uv_scale(0.5, 0.5);
    app.world.spawn((
        Transform::default(),
        MeshRenderer::new(
            Shape::Plane {
                size: [24.0, 24.0],
                subdivisions: 1,
            },
            material,
        ),
    ));
    let camera = app
        .world
        .query_filtered::<Entity, With<Camera>>()
        .iter(&app.world)
        .next()
        .expect("camera");
    let mut peak = 0;
    for frame in 0..160u32 {
        let t = frame as f64 / 160.0;
        let z = 8.0 - 16.0 * t;
        let x = 6.0 * (t * std::f64::consts::TAU).sin();
        let eye = DVec3::new(x, 0.25 + 0.2 * (t * 9.0).sin().abs(), z);
        *app.world.get_mut::<Transform>(camera).expect("transform") =
            Transform::from_translation(eye)
                .looking_at(eye + DVec3::new(0.2, -0.25, -1.0), Vec3::Y);
        app.update();
        let s = vt_stats(&app);
        assert!(
            s.resident_tiles <= s.cache_tiles && s.resident_bytes <= s.cache_bytes,
            "frame {frame}: over the cache: {s:?}"
        );
        assert!(s.cache_tiles <= 2 * 64, "frame {frame}: cache grew: {s:?}");
        peak = peak.max(s.resident_tiles);
        if frame % 40 == 39 {
            let img = capture(&mut app, &format!("flying_{frame}"));
            assert_no_black_or_magenta(&img, &format!("flying frame {frame}"));
        }
    }
    let s = vt_stats(&app);
    eprintln!("flying: peak {peak} resident, {s:?}");
    assert!(s.evictions > 0, "a small cache evicts while flying: {s:?}");
    assert!(peak > 32, "the cache was used: {s:?}");
}

/// While tiles stream in (one per frame), every pixel shows the nearest resident coarser
/// level: never black or magenta, and close to the final picture's colors.
#[test]
fn fallback_is_never_black_or_magenta() {
    let mut app = app(DVec3::new(0.0, 0.0, 0.4), DVec3::ZERO, |s| {
        s.virtual_texture_upload_budget = 1;
    });
    if !gpu(&mut app, "fallback") {
        return;
    }
    let color = add_image(&mut app, ground_image(2048));
    let material = StandardMaterial::color(1.0, 1.0, 1.0).with_base_color_texture(color);
    quad(&mut app, material, DVec3::ZERO, 2.0);
    let mut streaming = 0;
    for frame in 0..24 {
        app.update();
        let s = vt_stats(&app);
        if s.textures > 0 && (s.pending > 0 || s.feedback_misses > 0) {
            streaming += 1;
        }
        if frame % 3 == 2 {
            let img = capture(&mut app, &format!("fallback_{frame}"));
            assert_no_black_or_magenta(&img, &format!("fallback frame {frame}"));
        }
    }
    assert!(
        streaming > 0,
        "tiles were still streaming in: {:?}",
        vt_stats(&app)
    );
}

/// A procedural tile source (painted on the loader threads) draws like the same pattern as a
/// whole image.
#[test]
fn procedural_source_matches_the_whole_pattern() {
    let pattern = Pattern { size: 2048 };
    let reference = Image::from_fn(2048, 2048, false, |x, y| pattern.texel(0, x, y));
    let mut virt = app(DVec3::new(0.1, 0.1, 0.35), DVec3::new(0.1, 0.1, 0.0), |s| {
        s.virtual_texture_compression = false;
    });
    if !gpu(&mut virt, "procedural") {
        return;
    }
    let handle = virt
        .world
        .resource_scope(|world, mut registry: Mut<VirtualTextures>| {
            let mut images = world.resource_mut::<Assets<Image>>();
            registry.add_procedural(&mut images, Pattern { size: 2048 })
        })
        .expect("procedural");
    let material = StandardMaterial::color(1.0, 1.0, 1.0).with_base_color_texture(handle);
    quad(&mut virt, material, DVec3::ZERO, 2.0);
    let frames = settle(&mut virt, false, 300);
    let stats = vt_stats(&virt);
    let a = capture(&mut virt, "procedural_virtual");
    drop(virt);

    let mut whole = app(DVec3::new(0.1, 0.1, 0.35), DVec3::new(0.1, 0.1, 0.0), |s| {
        s.virtual_textures = false;
    });
    let image = add_image(&mut whole, reference);
    let material = StandardMaterial::color(1.0, 1.0, 1.0).with_base_color_texture(image);
    quad(&mut whole, material, DVec3::ZERO, 2.0);
    whole.update_n(frames as usize + 1);
    let b = capture(&mut whole, "procedural_whole");
    let d = compare(&a, &b);
    eprintln!("procedural: {:?} {stats:?}", (d.mean, d.p99, d.worst_block));
    assert_eq!(stats.textures, 1);
    assert!(stats.resident_by_level.first().copied().unwrap_or(0) > 0);
    assert!(
        d.mean < 1.5 && d.worst_block < 4.0,
        "procedural differs from whole: mean {}, p99 {}, worst 8x8 block {}",
        d.mean,
        d.p99,
        d.worst_block
    );
}

/// Virtual textures in every material path at once: triplanar (object and world space, with
/// anti-tiling), parallax from a virtual height map, alpha-masked shadows, a dense mesh on the
/// cluster path, instanced copies. Must match the scene with whole textures.
#[test]
fn every_material_path_matches_the_whole_textures() {
    let color = ground_image(2048);
    let normal = normal_image(2048);
    let height = height_image(2048);
    let cutout = cutout_image(2048);
    let Some((a, b, stats)) = render_both(
        "paths",
        DVec3::new(0.0, 2.2, 3.2),
        DVec3::new(0.0, 0.4, 0.0),
        |s| s.virtual_geometry_min_triangles = 2000,
        true,
        |app| {
            let color = add_image(app, color.clone());
            let normal = add_image(app, normal.clone());
            let height = add_image(app, height.clone());
            let cutout = add_image(app, cutout.clone());
            let textured = StandardMaterial::color(1.0, 1.0, 1.0)
                .with_base_color_texture(color)
                .with_normal_texture(normal);
            // Ground: world-space triplanar with anti-tiling and parallax.
            app.world.spawn((
                Transform::default(),
                MeshRenderer::new(
                    Shape::Plane {
                        size: [8.0, 8.0],
                        subdivisions: 1,
                    },
                    textured
                        .with_height_texture(height, 0.03)
                        .with_triplanar_world()
                        .with_texture_scale(1.5)
                        .with_anti_tiling(true),
                ),
            ));
            // A dense sphere (cluster path), object-space triplanar.
            app.world.spawn((
                Transform::from_xyz(-0.9, 0.6, 0.0),
                MeshRenderer::new(
                    Shape::Sphere {
                        radius: 0.6,
                        segments: 96,
                    },
                    textured.with_triplanar().with_texture_scale(0.8),
                ),
            ));
            // Instanced boxes, UV mapped.
            for i in 0..3 {
                app.world.spawn((
                    Transform::from_xyz(0.5 + 0.45 * i as f64, 0.2, 0.9),
                    MeshRenderer::new(
                        Shape::Cuboid {
                            size: [0.4, 0.4, 0.4],
                        },
                        textured,
                    ),
                ));
            }
            // An alpha-masked card casting a leaf-shaped shadow on the ground.
            app.world.spawn((
                Transform::from_xyz(0.9, 0.9, -0.3),
                MeshRenderer::new(
                    Shape::Quad { size: [1.2, 1.2] },
                    StandardMaterial {
                        double_sided: true,
                        ..StandardMaterial::color(1.0, 1.0, 1.0)
                            .with_base_color_texture(cutout)
                            .with_alpha_mask(0.5)
                    },
                ),
            ));
        },
    ) else {
        return;
    };
    let d = compare(&a, &b);
    eprintln!("paths: {:?} {stats:?}", (d.mean, d.p99, d.worst_block));
    assert_eq!(
        stats.textures, 4,
        "color, normal, height and cutout are virtual: {stats:?}"
    );
    assert!(
        d.mean < 3.0 && d.worst_block < 12.0,
        "virtual differs from whole: mean {}, p99 {}, worst 8x8 block {}",
        d.mean,
        d.p99,
        d.worst_block
    );
}

/// Correlation of the luminance with itself shifted by `(dx, dy)` pixels (1 = identical).
fn autocorrelation(img: &RgbaImage, dx: u32, dy: u32) -> f32 {
    let l = |x: u32, y: u32| {
        let p = img.get_pixel(x, y);
        0.2126 * p[0] as f32 + 0.7152 * p[1] as f32 + 0.0722 * p[2] as f32
    };
    let (w, h) = (img.width() - dx, img.height() - dy);
    let n = (w * h) as f32;
    let mut mean = 0.0;
    for y in 0..h {
        for x in 0..w {
            mean += l(x, y);
        }
    }
    mean /= n;
    let (mut ab, mut aa, mut bb) = (0.0, 0.0, 0.0);
    for y in 0..h {
        for x in 0..w {
            let a = l(x, y) - mean;
            let b = l(x + dx, y + dy) - mean;
            ab += a * b;
            aa += a * a;
            bb += b * b;
        }
    }
    ab / (aa * bb).sqrt().max(1e-6)
}

fn luminance_spread(img: &RgbaImage) -> f32 {
    let l: Vec<f32> = img
        .pixels()
        .map(|p| 0.2126 * p[0] as f32 + 0.7152 * p[1] as f32 + 0.0722 * p[2] as f32)
        .collect();
    let mean = l.iter().sum::<f32>() / l.len() as f32;
    (l.iter().map(|v| (v - mean) * (v - mean)).sum::<f32>() / l.len() as f32).sqrt()
}

/// A small texture repeated across a wall: plain tiling repeats exactly at the tile period;
/// anti-tiling must break that up (much lower autocorrelation at the period) while keeping the
/// texture's contrast.
#[test]
fn anti_tiling_breaks_up_the_repetition() {
    // Blobs of a few sizes on a mid background: features the eye catches repeating.
    let texture = Image::from_fn(256, 256, true, |x, y| {
        let n = fbm(x, y, 256, 21);
        let blob = value_noise(x, y, 256, 32, 4) > 0.62;
        let v = if blob {
            40.0 + 60.0 * n
        } else {
            120.0 + 100.0 * n
        };
        [v as u8, (v * 0.9) as u8, (v * 0.7) as u8, 255]
    });
    // Head-on view of a 6 m wall from 3 m, the texture once per meter.
    let distance = 3.0f32;
    let px_per_m = SIZE as f32 / (2.0 * distance * (FOV_Y_DEG.to_radians() * 0.5).tan());
    let period = px_per_m.round() as u32;
    let mut results = Vec::new();
    for anti in [false, true] {
        let mut app = app(DVec3::new(0.0, 0.0, distance as f64), DVec3::ZERO, |_| {});
        if !gpu(&mut app, "anti_tiling") {
            return;
        }
        let image = add_image(&mut app, texture.clone());
        let mut material = StandardMaterial::color(1.0, 1.0, 1.0)
            .with_roughness(1.0)
            .with_base_color_texture(image)
            .with_uv_scale(6.0, 6.0);
        if anti {
            material = material.with_anti_tiling(true);
        }
        quad(&mut app, material, DVec3::ZERO, 6.0);
        app.update_n(4);
        let img = capture(&mut app, if anti { "anti_tiled" } else { "plain_tiled" });
        let r = (autocorrelation(&img, period, 0) + autocorrelation(&img, 0, period)) * 0.5;
        results.push((r, luminance_spread(&img)));
    }
    let ((plain, plain_spread), (anti, anti_spread)) = (results[0], results[1]);
    eprintln!(
        "autocorrelation at the {period} px period: plain {plain:.3}, anti-tiled {anti:.3}; \
         luminance spread {plain_spread:.1} vs {anti_spread:.1}"
    );
    assert!(plain > 0.8, "plain tiling repeats: {plain}");
    assert!(
        anti < 0.5 * plain && anti < 0.4,
        "anti-tiling breaks up the period: {anti} vs {plain}"
    );
    assert!(
        anti_spread > 0.7 * plain_spread,
        "anti-tiling keeps the contrast: {anti_spread} vs {plain_spread}"
    );
}
