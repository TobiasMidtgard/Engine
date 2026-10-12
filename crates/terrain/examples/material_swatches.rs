//! Bake the ground materials and save a lit preview of each (plus its albedo, normal and
//! height maps) for reviewing them without the renderer. Scans are read from `scan_dir`
//! (default `assets/fetched/terrain`; `none` for procedural only).
//!
//! ```text
//! cargo run --release -p engine_terrain --example material_swatches -- [out_dir] [size] [scan_dir]
//! ```

use engine_terrain::materials::{GroundMaterial, bake};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let out = std::path::PathBuf::from(args.first().cloned().unwrap_or_else(|| "swatches".into()));
    let size: u32 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(1024);
    let scans = match args.get(2).map(String::as_str) {
        Some("none") => None,
        Some(dir) => Some(std::path::PathBuf::from(dir)),
        None => Some("assets/fetched/terrain".into()),
    };
    std::fs::create_dir_all(&out).expect("output folder");
    let baked = bake(size, scans.as_deref());
    println!(
        "baked {} materials at {size}² in {:.2}s, {} from scans",
        baked.stats.len(),
        baked.bake_secs,
        baked.scanned()
    );
    let n = size as usize;
    // Low sun from the upper left, a little sky fill.
    let sun = {
        let v = [-0.55f32, -0.45, 0.70];
        let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        [v[0] / l, v[1] / l, v[2] / l]
    };
    let mut sheet = image::RgbImage::new((n as u32 / 2) * 3, (n as u32 / 2) * 3);
    for (k, m) in GroundMaterial::ALL.iter().enumerate() {
        let a = &baked.albedo_height[k][0];
        let b = &baked.normal_rough_ao[k][0];
        let mut lit = image::RgbImage::new(size, size);
        let mut albedo = image::RgbImage::new(size, size);
        let mut normal = image::RgbImage::new(size, size);
        let mut height = image::GrayImage::new(size, size);
        for i in 0..n * n {
            let (x, y) = ((i % n) as u32, (i / n) as u32);
            let nx = b[i * 4] as f32 / 127.5 - 1.0;
            let ny = b[i * 4 + 1] as f32 / 127.5 - 1.0;
            let nz = (1.0 - nx * nx - ny * ny).max(0.0).sqrt();
            let ao = b[i * 4 + 3] as f32 / 255.0;
            let ndl = (nx * sun[0] + ny * sun[1] + nz * sun[2]).max(0.0);
            let light = ndl * 1.5 + 0.3 * ao;
            let px = |c: u8| {
                let lin = (c as f32 / 255.0).powf(2.2) * light;
                ((lin / (1.0 + lin)).powf(1.0 / 2.2) * 1.15 * 255.0).min(255.0) as u8
            };
            lit.put_pixel(
                x,
                y,
                image::Rgb([px(a[i * 4]), px(a[i * 4 + 1]), px(a[i * 4 + 2])]),
            );
            albedo.put_pixel(x, y, image::Rgb([a[i * 4], a[i * 4 + 1], a[i * 4 + 2]]));
            normal.put_pixel(
                x,
                y,
                image::Rgb([b[i * 4], b[i * 4 + 1], (nz * 255.0) as u8]),
            );
            height.put_pixel(x, y, image::Luma([a[i * 4 + 3]]));
            if x % 2 == 0 && y % 2 == 0 {
                let (sx, sy) = (
                    (k as u32 % 3) * size / 2 + x / 2,
                    (k as u32 / 3) * size / 2 + y / 2,
                );
                sheet.put_pixel(sx, sy, *lit.get_pixel(x, y));
            }
        }
        let name = m.name();
        lit.save(out.join(format!("{name}_lit.png"))).expect("save");
        albedo
            .save(out.join(format!("{name}_albedo.png")))
            .expect("save");
        normal
            .save(out.join(format!("{name}_normal.png")))
            .expect("save");
        height
            .save(out.join(format!("{name}_height.png")))
            .expect("save");
        let s = baked.stats[k];
        println!(
            "{name:<13} albedo {:.3} {:.3} {:.3}  roughness {:.2}  occlusion {:.2}  height {:.1} cm  {:?}",
            s.mean_albedo[0],
            s.mean_albedo[1],
            s.mean_albedo[2],
            s.mean_roughness,
            s.mean_occlusion,
            s.height_m * 100.0,
            baked.sources[k]
        );
    }
    sheet.save(out.join("sheet.png")).expect("save");
}
