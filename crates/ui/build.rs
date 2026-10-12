//! Embeds every SVG under `assets/icons` and `assets/portraits` so built-in icons work without
//! files on disk. Portraits are named `portrait:<file stem>`.

use std::fmt::Write;
use std::path::Path;

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets");
    let mut out = String::from(
        "/// Built-in SVG sources: (name, svg).\npub const BUILTIN_SVGS: &[(&str, &str)] = &[\n",
    );
    for (dir, prefix) in [("icons", ""), ("portraits", "portrait:")] {
        let path = root.join(dir);
        println!("cargo:rerun-if-changed={}", path.display());
        let Ok(entries) = std::fs::read_dir(&path) else {
            continue;
        };
        let mut files: Vec<_> = entries
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "svg"))
            .collect();
        files.sort();
        for file in files {
            println!("cargo:rerun-if-changed={}", file.display());
            let stem = file.file_stem().unwrap().to_string_lossy();
            writeln!(
                out,
                "    (\"{prefix}{stem}\", include_str!({:?})),",
                file.display().to_string()
            )
            .unwrap();
        }
    }
    out.push_str("];\n");
    let dest = Path::new(&std::env::var("OUT_DIR").unwrap()).join("builtin_svgs.rs");
    std::fs::write(dest, out).unwrap();
}
