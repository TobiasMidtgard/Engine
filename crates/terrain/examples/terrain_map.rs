//! Top-down shaded relief map of a planet around its home pad, for tuning shapes.
//!
//! ```text
//! cargo run --release -p engine_terrain --example terrain_map -- [out.png] [km across] [px]
//! ```

use engine_core::math::DVec3;
use engine_terrain::shape::{FlatPad, PlanetShape};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let out = args
        .first()
        .cloned()
        .unwrap_or_else(|| "terrain_map.png".into());
    let km: f64 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(160.0);
    let px: u32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(512);
    let pad = FlatPad::default();
    let shape = PlanetShape::earthlike(7).with_pad(pad);
    let c = pad.direction();
    let east = DVec3::Y.cross(c).normalize();
    let north = c.cross(east);
    let r = shape.radius_m;
    let step = km * 1000.0 / px as f64;
    let sample = |x: f64, y: f64| {
        let d = (c + east * (x / r) + north * (y / r)).normalize();
        shape.sample(d, step)
    };
    let mut img = image::RgbImage::new(px, px);
    let (mut lo, mut hi, mut land) = (f64::MAX, f64::MIN, 0u32);
    for j in 0..px {
        for i in 0..px {
            let x = (i as f64 - px as f64 * 0.5) * step;
            let y = (px as f64 * 0.5 - j as f64) * step;
            let s = sample(x, y);
            let h = s.height;
            lo = lo.min(h);
            hi = hi.max(h);
            let hx = sample(x + step, y).height - h;
            let hy = sample(x, y + step).height - h;
            // Light from the north-west, relief exaggerated 3×.
            let n = DVec3::new(-hx * 3.0, step, -hy * 3.0).normalize();
            let shade = (n.dot(DVec3::new(-0.6, 0.8, 0.4).normalize()) * 1.15).clamp(0.2, 1.25);
            let slope = 1.0 - (step / (hx * hx + hy * hy + step * step).sqrt());
            let col: [f64; 3] = if h < 0.0 {
                let deep = (-h / 3000.0).clamp(0.0, 1.0);
                [0.1 - 0.06 * deep, 0.3 - 0.17 * deep, 0.5 - 0.2 * deep]
            } else {
                land += 1;
                // Height tint: lowland green, upland olive, high brown, then snow and rock.
                let t = (h / 2500.0).clamp(0.0, 1.0);
                let low = if s.moisture > 0.5 {
                    [0.16, 0.34, 0.12]
                } else {
                    [0.36, 0.44, 0.2]
                };
                let tint = [
                    low[0] + (0.5 - low[0]) * t,
                    low[1] + (0.42 - low[1]) * t,
                    low[2] + (0.3 - low[2]) * t,
                ];
                if s.temperature_c < -2.0 && slope < 0.5 {
                    [0.92, 0.94, 0.97]
                } else if slope > 0.35 {
                    [0.45, 0.42, 0.4]
                } else if h < 4.0 {
                    [0.75, 0.68, 0.5]
                } else {
                    tint
                }
            };
            let k = if h < 0.0 { 1.0 } else { shade };
            img.put_pixel(
                i,
                j,
                image::Rgb(col.map(|v| ((v * k).clamp(0.0, 1.0) * 255.0) as u8)),
            );
        }
    }
    // Mark the pad.
    for d in 0..6 {
        img.put_pixel(px / 2 + d, px / 2, image::Rgb([255, 0, 0]));
        img.put_pixel(px / 2, px / 2 + d, image::Rgb([255, 0, 0]));
    }
    img.save(&out).expect("png");
    println!(
        "{out}: {km} km, heights {lo:.0}..{hi:.0} m, land {:.0}%",
        land as f64 / (px * px) as f64 * 100.0
    );
}
