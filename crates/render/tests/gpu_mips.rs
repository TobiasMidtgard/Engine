//! Large images get their mip chain drawn on the GPU (`mipmaps.rs`) instead of made on the main
//! thread: the levels match the CPU chain (filtered in linear light for sRGB images), and the
//! upload no longer pays for the chain. Needs a GPU or lavapipe; passes with a note when neither
//! is available.

use engine_assets::Assets;
use engine_core::prelude::*;
use engine_render::prelude::*;
use engine_render::{Image, RenderDisabled, Renderer};

const SIZE: u32 = 1024;
/// The level compared (64²).
const LEVEL: u32 = 4;

/// Fine stripes of saturated colors over a gradient: averaging them in linear light or in sRGB
/// gives clearly different levels.
fn pattern() -> Image {
    let mut data = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    for y in 0..SIZE {
        for x in 0..SIZE {
            let stripe = (x / 2 + y / 3) % 3;
            let g = (x * 255 / SIZE) as u8;
            let px = match stripe {
                0 => [255, 20, g, 255],
                1 => [10, 230, 40, 255],
                _ => [g, 0, 255 - g, 255],
            };
            data.extend_from_slice(&px);
        }
    }
    Image::new(SIZE, SIZE, data, true)
}

fn read_level(renderer: &Renderer, texture: &wgpu::Texture, level: u32) -> Vec<u8> {
    let device = &renderer.shared.device;
    let size = (SIZE >> level).max(1);
    let row = (size * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: (row * size) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: level,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row),
                rows_per_image: Some(size),
            },
        },
        wgpu::Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: 1,
        },
    );
    renderer.shared.queue.submit([encoder.finish()]);
    buffer.slice(..).map_async(wgpu::MapMode::Read, |_| {});
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll");
    let data = buffer
        .slice(..)
        .get_mapped_range()
        .expect("mapped")
        .to_vec();
    let mut out = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size as usize {
        out.extend_from_slice(&data[y * row as usize..y * row as usize + size as usize * 4]);
    }
    out
}

#[test]
fn a_large_images_chain_is_drawn_on_the_gpu_and_matches_the_cpu_one() {
    engine_core::log::init("error");
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    app.add_plugins(RenderPlugin);
    // Keep the chain exact (no block compression) to compare it.
    app.world
        .resource_mut::<RenderSettings>()
        .texture_compression = TextureCompression::Off;
    app.world
        .spawn((Camera::perspective(60.0), Transform::default()));
    app.update();
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return;
    }
    let image = pattern();
    let started = std::time::Instant::now();
    let cpu_chain = image.mip_chain(true);
    let cpu_ms = started.elapsed().as_secs_f64() * 1000.0;
    let handle = app.world.resource_mut::<Assets<Image>>().add(image);

    let started = std::time::Instant::now();
    let texture = {
        let images = app.world.resource::<Assets<Image>>();
        app.world
            .resource::<Renderer>()
            .shared
            .image_texture(images, handle, true)
            .expect("uploaded")
    };
    let upload_ms = started.elapsed().as_secs_f64() * 1000.0;
    assert_eq!(texture.mip_levels, SIZE.ilog2() + 1);
    // The chain goes with the next frame's submit.
    app.update();
    let gpu = read_level(app.world.resource::<Renderer>(), texture.texture(), LEVEL);
    let cpu = &cpu_chain[LEVEL as usize - 1];
    assert_eq!(gpu.len(), cpu.len());
    let diff = gpu
        .iter()
        .zip(cpu)
        .map(|(a, b)| (*a as f32 - *b as f32).abs())
        .sum::<f32>()
        / gpu.len() as f32;
    eprintln!(
        "level {LEVEL}: mean difference {diff:.2} of 255; upload {upload_ms:.1} ms, the CPU \
         chain alone {cpu_ms:.1} ms"
    );
    assert!(diff < 3.0, "GPU and CPU chains differ by {diff:.2}");
    // Averaged in linear light: the stripes' mix is brighter than an sRGB-space average.
    let mean = |v: &[u8]| v.iter().map(|&x| x as f32).sum::<f32>() / v.len() as f32;
    assert!((mean(&gpu) - mean(cpu)).abs() < 2.0);
}
