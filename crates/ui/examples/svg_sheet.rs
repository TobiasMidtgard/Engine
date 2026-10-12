//! Render every `.svg` in a folder into one contact sheet, to review icon and portrait art.
//!
//! ```sh
//! cargo run -p engine_ui --example svg_sheet -- crates/ui/assets/icons sheet.png [cell_px] [background_hex]
//! ```

use resvg::{tiny_skia, usvg};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let dir = args
        .get(1)
        .map(String::as_str)
        .unwrap_or("crates/ui/assets/icons");
    let out = args.get(2).map(String::as_str).unwrap_or("sheet.png");
    let cell: u32 = args.get(3).and_then(|v| v.parse().ok()).unwrap_or(128);
    let bg = args
        .get(4)
        .and_then(|v| u32::from_str_radix(v.trim_start_matches('#'), 16).ok())
        .unwrap_or(0x1b2230);

    let mut files: Vec<_> = std::fs::read_dir(dir)
        .expect("read dir")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "svg"))
        .collect();
    files.sort();
    let cols = (files.len() as f32).sqrt().ceil().max(1.0) as u32;
    let rows = (files.len() as u32).div_ceil(cols).max(1);
    let pad = 8;
    let (w, h) = (cols * (cell + pad) + pad, rows * (cell + pad) + pad);
    let mut sheet = tiny_skia::Pixmap::new(w, h).expect("pixmap");
    sheet.fill(tiny_skia::Color::from_rgba8(
        (bg >> 16) as u8,
        (bg >> 8) as u8,
        bg as u8,
        255,
    ));
    let opt = usvg::Options::default();
    for (i, path) in files.iter().enumerate() {
        let data = std::fs::read(path).expect("read svg");
        let tree = match usvg::Tree::from_data(&data, &opt) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("{}: {e}", path.display());
                continue;
            }
        };
        let size = tree.size();
        let s = (cell as f32 / size.width()).min(cell as f32 / size.height());
        let (c, r) = (i as u32 % cols, i as u32 / cols);
        let x = (pad + c * (cell + pad)) as f32 + (cell as f32 - size.width() * s) * 0.5;
        let y = (pad + r * (cell + pad)) as f32 + (cell as f32 - size.height() * s) * 0.5;
        let transform = tiny_skia::Transform::from_row(s, 0.0, 0.0, s, x, y);
        resvg::render(&tree, transform, &mut sheet.as_mut());
        println!("{:>3} {}", i, path.file_name().unwrap().to_string_lossy());
    }
    sheet.save_png(out).expect("save png");
    println!("wrote {out} ({w}x{h})");
}
