//! Unpacking zip archives into the folder they were fetched to.
//!
//! Some hosts (ambientCG, ShareTextures, cgbookcase) serve a material as one zip. An entry
//! with `unpack` set is fetched and pinned as the archive itself, then unpacked next to it:
//!
//! - `unpack = "zip"` extracts every file in the archive as it is.
//! - `unpack = "pbr"` reads a PBR texture set out of the archive and writes it in the engine's
//!   scan layout, the one Poly Haven uses and the terrain and foliage loaders read:
//!   `<slug>_diff_<res>.jpg` (color; `.png` with the opacity in alpha when the set has an
//!   opacity map, as leaf atlases do), `<slug>_nor_gl_<res>.jpg` (OpenGL normal, +Y up; DirectX
//!   normals are flipped), `<slug>_arm_<res>.jpg` (R occlusion, G roughness, B metalness, glTF
//!   order; missing maps are white, mid-rough and black) and `<slug>_disp_<res>.jpg` (height).
//!   `<slug>` is the name of the folder the archive sits in and `<res>` comes from the image
//!   width (`1k`, `2k`, `4k`, `8k`).
//!
//! The outputs are made from the pinned archive by fixed code, so they are not pinned
//! themselves; a marker file next to the archive records which archive and mode made them,
//! and unpacking runs again only when that changes or an output is missing.

use std::fs;
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};

use image::{DynamicImage, GrayImage, ImageBuffer, ImageEncoder, Rgb, RgbImage, RgbaImage};

/// Bumped when the outputs of an unpack mode change, so existing folders are redone.
const VERSION: u32 = 1;

/// The `unpack` modes an entry may name.
pub const UNPACK_MODES: &[&str] = &["zip", "pbr"];

/// JPEG quality of the files `pbr` writes (Poly Haven ships about 90-95).
const JPEG_QUALITY: u8 = 92;

/// What a map in a PBR archive is, read from its file name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MapKind {
    Color,
    NormalGl,
    NormalDx,
    Roughness,
    Occlusion,
    Metalness,
    Height,
    Opacity,
    /// Already packed occlusion, roughness, metalness.
    Arm,
}

/// The map a file holds, from the last `_`, `-` or space separated words of its stem
/// (`Ground054_2K-JPG_NormalGL.jpg`, `bark_oak_basecolor.png`, `Moss 02 AO.jpg`). `None` for
/// previews, `.mtlx`, `.usdc` and anything else that isn't a known map.
pub fn map_kind(file_name: &str) -> Option<MapKind> {
    let path = Path::new(file_name);
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    if !matches!(ext.as_str(), "jpg" | "jpeg" | "png" | "tif" | "tiff") {
        return None;
    }
    let stem = path.file_stem()?.to_str()?.to_ascii_lowercase();
    let mut words: Vec<&str> = stem
        .split(['_', '-', ' ', '.'])
        .filter(|w| !w.is_empty())
        .collect();
    // A trailing resolution (`rock_face_nor_gl_2k`) is not part of the map name.
    let is_res = |w: &str| {
        w.len() >= 2 && w.ends_with('k') && w[..w.len() - 1].bytes().all(|b| b.is_ascii_digit())
    };
    while words.last().is_some_and(|w| is_res(w)) {
        words.pop();
    }
    let last = *words.last()?;
    let last_two = if words.len() >= 2 {
        format!("{}{}", words[words.len() - 2], last)
    } else {
        String::new()
    };
    let kind = |w: &str| -> Option<MapKind> {
        Some(match w {
            "color" | "colour" | "basecolor" | "albedo" | "diffuse" | "diff" | "col" => {
                MapKind::Color
            }
            "normalgl" | "norgl" | "normal" | "nor" | "nrm" => MapKind::NormalGl,
            "normaldx" | "nordx" => MapKind::NormalDx,
            "roughness" | "rough" | "rgh" => MapKind::Roughness,
            "ambientocclusion" | "ao" | "occlusion" => MapKind::Occlusion,
            "metalness" | "metallic" | "metal" => MapKind::Metalness,
            "displacement" | "height" | "disp" => MapKind::Height,
            "opacity" | "alpha" | "mask" => MapKind::Opacity,
            "arm" => MapKind::Arm,
            _ => return None,
        })
    };
    // Two-word names first ("base color", "normal gl", "ambient occlusion").
    kind(&last_two).or_else(|| kind(last))
}

/// The resolution word of the scan layout names for an image `width` texels wide.
fn resolution_word(width: u32) -> String {
    match width {
        0..=1536 => "1k".into(),
        1537..=3072 => "2k".into(),
        3073..=6144 => "4k".into(),
        _ => "8k".into(),
    }
}

/// The marker that records what unpacked a folder.
fn marker_path(archive: &Path) -> PathBuf {
    let mut name = archive.file_name().unwrap_or_default().to_os_string();
    name.push(".unpacked");
    archive.with_file_name(name)
}

/// Whether `archive` (pinned to `sha256`) still has to be unpacked in `mode`: the marker is
/// missing or stale, or a file it lists is gone.
pub fn needs_unpack(archive: &Path, sha256: &str, mode: &str) -> bool {
    let Ok(text) = fs::read_to_string(marker_path(archive)) else {
        return true;
    };
    let mut lines = text.lines();
    if lines.next() != Some(&format!("{mode} {VERSION} {sha256}")) {
        return true;
    }
    let dir = archive.parent().unwrap_or(Path::new("."));
    lines.any(|name| !dir.join(name).is_file())
}

/// Unpack `archive` (pinned to `sha256`) in `mode` into its own folder. Returns the names of
/// the files written, relative to that folder.
pub fn unpack(archive: &Path, sha256: &str, mode: &str) -> Result<Vec<String>, String> {
    let dir = archive.parent().unwrap_or(Path::new(".")).to_path_buf();
    let file =
        fs::File::open(archive).map_err(|e| format!("can't open {}: {e}", archive.display()))?;
    let mut zip = zip::ZipArchive::new(io::BufReader::new(file))
        .map_err(|e| format!("{} is not a readable zip: {e}", archive.display()))?;
    let written = match mode {
        "zip" => extract_all(&mut zip, &dir)?,
        "pbr" => {
            let slug = dir
                .file_name()
                .and_then(|s| s.to_str())
                .ok_or("a pbr archive needs a folder of its own to name the set")?
                .to_string();
            write_pbr(&mut zip, &dir, &slug)?
        }
        other => return Err(format!("unknown unpack mode {other:?}")),
    };
    let mut marker = format!("{mode} {VERSION} {sha256}\n");
    for name in &written {
        marker.push_str(name);
        marker.push('\n');
    }
    fs::write(marker_path(archive), marker).map_err(|e| e.to_string())?;
    Ok(written)
}

/// A member name as a safe relative path (no `..`, no root, forward slashes).
fn safe_member(name: &str) -> Option<PathBuf> {
    let p = Path::new(name);
    if p.components().all(|c| matches!(c, Component::Normal(_))) {
        Some(p.to_path_buf())
    } else {
        None
    }
}

fn extract_all<R: io::Read + io::Seek>(
    zip: &mut zip::ZipArchive<R>,
    dir: &Path,
) -> Result<Vec<String>, String> {
    let mut written = Vec::new();
    for i in 0..zip.len() {
        let mut member = zip.by_index(i).map_err(|e| e.to_string())?;
        if member.is_dir() {
            continue;
        }
        // `enclosed_name` refuses absolute paths and `..` (no zip-slip); the check after it
        // keeps the result to plain names on every OS.
        let Some(rel) = member
            .enclosed_name()
            .and_then(|p| safe_member(&p.to_string_lossy()))
        else {
            return Err(format!(
                "archive member {:?} leaves its folder",
                member.name()
            ));
        };
        let out = dir.join(&rel);
        if let Some(parent) = out.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let mut bytes = Vec::new();
        member.read_to_end(&mut bytes).map_err(|e| e.to_string())?;
        fs::write(&out, bytes).map_err(|e| format!("can't write {}: {e}", out.display()))?;
        written.push(rel.to_string_lossy().replace('\\', "/"));
    }
    written.sort();
    Ok(written)
}

/// Read every map of a PBR set out of the archive (the largest image of each kind wins, so a
/// zip with both 1K and 2K maps gives the 2K set).
fn read_maps<R: io::Read + io::Seek>(
    zip: &mut zip::ZipArchive<R>,
) -> Result<Vec<(MapKind, DynamicImage)>, String> {
    let mut maps: Vec<(MapKind, DynamicImage)> = Vec::new();
    for i in 0..zip.len() {
        let mut member = zip.by_index(i).map_err(|e| e.to_string())?;
        if member.is_dir() {
            continue;
        }
        let name = member
            .name()
            .rsplit('/')
            .next()
            .unwrap_or_default()
            .to_string();
        let Some(kind) = map_kind(&name) else {
            continue;
        };
        let mut bytes = Vec::new();
        member.read_to_end(&mut bytes).map_err(|e| e.to_string())?;
        let img = image::load_from_memory(&bytes).map_err(|e| format!("{name}: {e}"))?;
        match maps.iter_mut().find(|(k, _)| *k == kind) {
            Some(slot) if slot.1.width() < img.width() => slot.1 = img,
            Some(_) => {}
            None => maps.push((kind, img)),
        }
    }
    Ok(maps)
}

fn write_pbr<R: io::Read + io::Seek>(
    zip: &mut zip::ZipArchive<R>,
    dir: &Path,
    slug: &str,
) -> Result<Vec<String>, String> {
    let maps = read_maps(zip)?;
    let get = |k: MapKind| maps.iter().find(|(kind, _)| *kind == k).map(|(_, i)| i);
    let color = get(MapKind::Color).ok_or("the archive has no color map")?;
    let (w, h) = (color.width(), color.height());
    let res = resolution_word(w);
    let name = |map: &str, ext: &str| format!("{slug}_{map}_{res}.{ext}");
    let mut written = Vec::new();

    // Color, with the opacity in alpha when there is one.
    if let Some(opacity) = get(MapKind::Opacity) {
        let alpha = gray_at(opacity, w, h);
        let rgb = color.to_rgb8();
        let rgba = RgbaImage::from_fn(w, h, |x, y| {
            let c = rgb.get_pixel(x, y);
            image::Rgba([c[0], c[1], c[2], alpha.get_pixel(x, y)[0]])
        });
        let out = name("diff", "png");
        rgba.save(dir.join(&out))
            .map_err(|e| format!("{out}: {e}"))?;
        written.push(out);
    } else {
        let out = name("diff", "jpg");
        save_jpeg(&color.to_rgb8(), &dir.join(&out))?;
        written.push(out);
    }

    // Normal, OpenGL convention.
    let normal = match (get(MapKind::NormalGl), get(MapKind::NormalDx)) {
        (Some(gl), _) => Some(rgb_at(gl, w, h)),
        (None, Some(dx)) => {
            let mut n = rgb_at(dx, w, h);
            for p in n.pixels_mut() {
                p[1] = 255 - p[1];
            }
            Some(n)
        }
        (None, None) => None,
    };
    if let Some(n) = normal {
        let out = name("nor_gl", "jpg");
        save_jpeg(&n, &dir.join(&out))?;
        written.push(out);
    }

    // Occlusion, roughness, metalness.
    let arm = if let Some(arm) = get(MapKind::Arm) {
        Some(rgb_at(arm, w, h))
    } else {
        let ao = get(MapKind::Occlusion).map(|i| gray_at(i, w, h));
        let rough = get(MapKind::Roughness).map(|i| gray_at(i, w, h));
        let metal = get(MapKind::Metalness).map(|i| gray_at(i, w, h));
        (ao.is_some() || rough.is_some() || metal.is_some()).then(|| {
            RgbImage::from_fn(w, h, |x, y| {
                let at = |m: &Option<GrayImage>, default: u8| {
                    m.as_ref().map_or(default, |m| m.get_pixel(x, y)[0])
                };
                Rgb([at(&ao, 255), at(&rough, 200), at(&metal, 0)])
            })
        })
    };
    if let Some(arm) = arm {
        let out = name("arm", "jpg");
        save_jpeg(&arm, &dir.join(&out))?;
        written.push(out);
    }

    // Height.
    if let Some(height) = get(MapKind::Height) {
        let g = gray_at(height, w, h);
        let rgb = RgbImage::from_fn(w, h, |x, y| {
            let v = g.get_pixel(x, y)[0];
            Rgb([v, v, v])
        });
        let out = name("disp", "jpg");
        save_jpeg(&rgb, &dir.join(&out))?;
        written.push(out);
    }
    Ok(written)
}

/// `img` as 8-bit gray at `w`×`h` (maps of another size are resampled to the color's size).
fn gray_at(img: &DynamicImage, w: u32, h: u32) -> GrayImage {
    let g = img.to_luma8();
    if g.dimensions() == (w, h) {
        g
    } else {
        image::imageops::resize(&g, w, h, image::imageops::FilterType::Triangle)
    }
}

fn rgb_at(img: &DynamicImage, w: u32, h: u32) -> RgbImage {
    let c = img.to_rgb8();
    if c.dimensions() == (w, h) {
        c
    } else {
        image::imageops::resize(&c, w, h, image::imageops::FilterType::Triangle)
    }
}

fn save_jpeg(img: &ImageBuffer<Rgb<u8>, Vec<u8>>, out: &Path) -> Result<(), String> {
    let file = fs::File::create(out).map_err(|e| format!("can't write {}: {e}", out.display()))?;
    image::codecs::jpeg::JpegEncoder::new_with_quality(io::BufWriter::new(file), JPEG_QUALITY)
        .write_image(
            img.as_raw(),
            img.width(),
            img.height(),
            image::ExtendedColorType::Rgb8,
        )
        .map_err(|e| format!("can't encode {}: {e}", out.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn map_names_from_the_usual_hosts() {
        use MapKind::*;
        let cases = [
            ("Ground054_2K-JPG_Color.jpg", Some(Color)),
            ("Ground054_2K-JPG_NormalGL.jpg", Some(NormalGl)),
            ("Ground054_2K-JPG_NormalDX.jpg", Some(NormalDx)),
            ("Ground054_2K-JPG_Roughness.jpg", Some(Roughness)),
            ("Ground054_2K-JPG_AmbientOcclusion.jpg", Some(Occlusion)),
            ("Ground054_2K-JPG_Displacement.jpg", Some(Height)),
            ("LeafSet007_2K-JPG_Opacity.jpg", Some(Opacity)),
            ("Metal012_2K-JPG_Metalness.jpg", Some(Metalness)),
            ("bark_oak_base_color.png", Some(Color)),
            ("Moss 02 AO.jpg", Some(Occlusion)),
            ("rock_face_nor_gl_2k.jpg", Some(NormalGl)),
            ("rock_face_arm_2k.jpg", Some(Arm)),
            ("Ground054.png", None),
            ("Ground054_2K-JPG.mtlx", None),
            ("Ground054_2K-JPG.usdc", None),
        ];
        for (name, want) in cases {
            assert_eq!(map_kind(name), want, "{name}");
        }
    }

    fn jpg(img: &RgbImage) -> Vec<u8> {
        let mut out = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 95)
            .write_image(
                img.as_raw(),
                img.width(),
                img.height(),
                image::ExtendedColorType::Rgb8,
            )
            .unwrap();
        out
    }

    fn write_zip(path: &Path, members: &[(&str, Vec<u8>)]) {
        let mut zip = zip::ZipWriter::new(fs::File::create(path).unwrap());
        for (name, bytes) in members {
            zip.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(bytes).unwrap();
        }
        zip.finish().unwrap();
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("fetch-assets-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_pbr_archive_becomes_the_scan_layout() {
        let root = temp_dir("pbr");
        let dir = root.join("moss");
        fs::create_dir_all(&dir).unwrap();
        let n = 2048;
        let flat = |c: [u8; 3]| jpg(&RgbImage::from_pixel(n, n, Rgb(c)));
        let archive = dir.join("Moss001_2K-JPG.zip");
        write_zip(
            &archive,
            &[
                ("Moss001_2K-JPG_Color.jpg", flat([60, 90, 30])),
                // DirectX normal tilted toward -Y: flipped to +Y in the GL output.
                ("Moss001_2K-JPG_NormalDX.jpg", flat([128, 40, 255])),
                ("Moss001_2K-JPG_Roughness.jpg", flat([220, 220, 220])),
                // A smaller occlusion map is resampled to the color's size.
                (
                    "Moss001_2K-JPG_AmbientOcclusion.jpg",
                    jpg(&RgbImage::from_pixel(512, 512, Rgb([100, 100, 100]))),
                ),
                ("Moss001_2K-JPG_Displacement.jpg", flat([77, 77, 77])),
                ("Moss001.png", vec![1, 2, 3]),
                ("Moss001_2K-JPG.mtlx", b"<materialx/>".to_vec()),
            ],
        );
        let sha = "a".repeat(64);
        assert!(needs_unpack(&archive, &sha, "pbr"));
        let written = unpack(&archive, &sha, "pbr").unwrap();
        assert_eq!(
            written,
            [
                "moss_diff_2k.jpg",
                "moss_nor_gl_2k.jpg",
                "moss_arm_2k.jpg",
                "moss_disp_2k.jpg"
            ]
        );
        let near = |a: u8, b: u8| (a as i32 - b as i32).abs() <= 3;
        let px = |f: &str| {
            image::open(dir.join(f))
                .unwrap()
                .to_rgb8()
                .get_pixel(7, 9)
                .0
        };
        let c = px("moss_diff_2k.jpg");
        assert!(near(c[0], 60) && near(c[1], 90) && near(c[2], 30), "{c:?}");
        let nrm = px("moss_nor_gl_2k.jpg");
        assert!(near(nrm[1], 215), "green flipped: {nrm:?}");
        let arm = px("moss_arm_2k.jpg");
        assert!(
            near(arm[0], 100) && near(arm[1], 220) && arm[2] <= 3,
            "{arm:?}"
        );
        assert_eq!(image::open(dir.join("moss_arm_2k.jpg")).unwrap().width(), n);

        // Unpacked once; a missing output or another archive pin makes it run again.
        assert!(!needs_unpack(&archive, &sha, "pbr"));
        assert!(needs_unpack(&archive, &"b".repeat(64), "pbr"));
        fs::remove_file(dir.join("moss_disp_2k.jpg")).unwrap();
        assert!(needs_unpack(&archive, &sha, "pbr"));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn an_opacity_map_goes_into_the_color_alpha() {
        let root = temp_dir("leaves");
        let dir = root.join("leaf_set");
        fs::create_dir_all(&dir).unwrap();
        let mut mask = RgbImage::from_pixel(1024, 1024, Rgb([0, 0, 0]));
        for x in 0..512 {
            for y in 0..1024 {
                mask.put_pixel(x, y, Rgb([255, 255, 255]));
            }
        }
        let archive = dir.join("LeafSet001_1K-JPG.zip");
        write_zip(
            &archive,
            &[
                (
                    "LeafSet001_1K-JPG_Color.jpg",
                    jpg(&RgbImage::from_pixel(1024, 1024, Rgb([40, 120, 30]))),
                ),
                ("LeafSet001_1K-JPG_Opacity.jpg", jpg(&mask)),
            ],
        );
        let written = unpack(&archive, &"c".repeat(64), "pbr").unwrap();
        assert_eq!(written, ["leaf_set_diff_1k.png"]);
        let img = image::open(dir.join("leaf_set_diff_1k.png"))
            .unwrap()
            .to_rgba8();
        assert!(img.get_pixel(100, 500)[3] > 250);
        assert!(img.get_pixel(900, 500)[3] < 5);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn plain_zips_extract_and_never_leave_their_folder() {
        let root = temp_dir("zip");
        let archive = root.join("model.zip");
        write_zip(
            &archive,
            &[
                ("model.gltf", b"{}".to_vec()),
                ("textures/a.jpg", vec![9; 10]),
            ],
        );
        let written = unpack(&archive, &"d".repeat(64), "zip").unwrap();
        assert_eq!(written, ["model.gltf", "textures/a.jpg"]);
        assert_eq!(fs::read(root.join("textures/a.jpg")).unwrap(), vec![9; 10]);

        let evil = root.join("evil.zip");
        write_zip(&evil, &[("../outside.txt", b"x".to_vec())]);
        assert!(unpack(&evil, &"e".repeat(64), "zip").is_err());
        assert!(!root.parent().unwrap().join("outside.txt").exists());
        let _ = fs::remove_dir_all(&root);
    }
}
