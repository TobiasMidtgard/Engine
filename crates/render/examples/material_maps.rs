//! Generate material presets on the GPU and save their maps as contact sheets (base color,
//! normal, occlusion/roughness/metalness, height, and a quick raking-light preview).
//!
//! ```sh
//! cargo run -p engine_render --example material_maps -- <out-dir> [size] [preset names...]
//! ```

use engine_core::prelude::*;
use engine_render::prelude::*;
use engine_render::{MaterialMaps, MaterialPreset, Renderer};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let out = std::path::PathBuf::from(
        args.first()
            .cloned()
            .unwrap_or_else(|| "material_maps".into()),
    );
    let size: u32 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(1024);
    let wanted: Vec<String> = args.iter().skip(2).map(|s| s.to_lowercase()).collect();
    std::fs::create_dir_all(&out).expect("output directory");

    engine_core::log::init("warn");
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = 64;
        w.height = 64;
    }
    app.add_plugins(RenderPlugin);
    app.update();
    let renderer = app.world.resource::<Renderer>();
    for preset in MaterialPreset::ALL {
        let name = preset.name().replace(' ', "_");
        if !wanted.is_empty() && !wanted.iter().any(|w| *w == name || *w == preset.name()) {
            continue;
        }
        let started = std::time::Instant::now();
        let maps = renderer
            .shared
            .read_material_maps(preset, size)
            .expect("generate");
        println!(
            "{name}: {:.2} s, non-finite texels {}, clamped albedo texels {}",
            started.elapsed().as_secs_f64(),
            maps.non_finite_texels,
            maps.clamped_texels
        );
        sheet(&maps)
            .save(out.join(format!("{name}.png")))
            .expect("save");
        lit(&maps, size)
            .save(out.join(format!("{name}_lit.png")))
            .expect("save");
    }
}

fn rgba(img: &Image, x: u32, y: u32) -> [u8; 4] {
    img.pixel(x, y)
}

fn to_linear(c: u8) -> f32 {
    let s = c as f32 / 255.0;
    if s <= 0.04045 {
        s / 12.92
    } else {
        ((s + 0.055) / 1.055).powf(2.4)
    }
}

fn to_srgb(l: f32) -> u8 {
    let l = l.clamp(0.0, 1.0);
    let s = if l <= 0.0031308 {
        l * 12.92
    } else {
        1.055 * l.powf(1.0 / 2.4) - 0.055
    };
    (s * 255.0 + 0.5) as u8
}

/// Raking-light preview: sun 25 degrees above the surface from the upper left, sky light
/// scaled by the occlusion map, a little specular from roughness.
fn shade(maps: &MaterialMaps, x: u32, y: u32) -> [u8; 4] {
    let a = rgba(&maps.base_color, x, y);
    let n = rgba(&maps.normal, x, y);
    let o = rgba(&maps.orm, x, y);
    let albedo = [to_linear(a[0]), to_linear(a[1]), to_linear(a[2])];
    let nv = Vec3::new(
        n[0] as f32 / 127.5 - 1.0,
        n[1] as f32 / 127.5 - 1.0,
        n[2] as f32 / 127.5 - 1.0,
    )
    .normalize_or(Vec3::Z);
    let l = Vec3::new(-0.65, 0.55, 0.42).normalize();
    let v = Vec3::Z;
    let ao = o[0] as f32 / 255.0;
    let rough = (o[1] as f32 / 255.0).max(0.05);
    let metal = o[2] as f32 / 255.0;
    let ndl = nv.dot(l).max(0.0);
    let h = (l + v).normalize();
    let a2 = rough.powi(4);
    let ndh = nv.dot(h).max(0.0);
    let d = a2 / (std::f32::consts::PI * ((ndh * ndh) * (a2 - 1.0) + 1.0).powi(2));
    let mut out = [0u8; 4];
    for c in 0..3 {
        let f0 = 0.04 * (1.0 - metal) + albedo[c] * metal;
        let diffuse = albedo[c] * (1.0 - metal);
        let lit = diffuse * (ndl * 2.4 + 0.35 * ao) + f0 * (d * ndl * 0.6 + 0.2 * ao);
        out[c] = to_srgb(lit / (1.0 + lit * 0.3));
    }
    out[3] = 255;
    out
}

fn lit(maps: &MaterialMaps, size: u32) -> image::RgbaImage {
    image::RgbaImage::from_fn(size, size, |x, y| image::Rgba(shade(maps, x, y)))
}

/// 3 x 2 sheet at 512 texels per cell: base color, lit, normal / ORM, height, roughness.
fn sheet(maps: &MaterialMaps) -> image::RgbaImage {
    let size = maps.base_color.width;
    let cell = 512u32.min(size);
    let step = (size / cell).max(1);
    image::RgbaImage::from_fn(cell * 3, cell * 2, |x, y| {
        let (cx, cy) = (x / cell, y / cell);
        let (sx, sy) = ((x % cell) * step, (y % cell) * step);
        let p = match (cx, cy) {
            (0, 0) => rgba(&maps.base_color, sx, sy),
            (1, 0) => shade(maps, sx, sy),
            (2, 0) => rgba(&maps.normal, sx, sy),
            (0, 1) => rgba(&maps.orm, sx, sy),
            (1, 1) => {
                let h = rgba(&maps.height, sx, sy)[0];
                [h, h, h, 255]
            }
            _ => {
                let r = rgba(&maps.orm, sx, sy)[1];
                [r, r, r, 255]
            }
        };
        image::Rgba([p[0], p[1], p[2], 255])
    })
}
