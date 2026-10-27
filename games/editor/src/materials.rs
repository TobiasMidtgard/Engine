//! Texture sets become materials, and materials go onto objects.
//!
//! Drop a folder (or several image files) exported from Substance, Quixel, Poly Haven,
//! ambientCG or Blender, and the maps are matched by the words in their names, the way Blender's
//! Node Wrangler sets up a Principled material ("diffuse diff albedo base col color", "roughness
//! rough rgh", "normal nor nrm nrml norm", ...): `Cliff_Rock_BaseColor.png`,
//! `rock_face_nor_gl_2k.jpg`, `Ground054_2K-PNG_Roughness.png` and `Mud_2K_Gloss.jpg` all find
//! their slot, and one folder can hold several sets. What a set has is repacked into glTF's
//! layout: separate roughness, gloss, metallic and occlusion maps (and Unity's mask and
//! metallic-smoothness maps) become one occlusion-roughness-metallic image, DirectX normal maps
//! get their green flipped, an opacity map goes into the color's alpha. Packed ORM / ARM maps
//! (Substance's Unreal preset, Poly Haven) are used as they are.
//!
//! Each set is written as `imported/materials/<name>/<name>.material.gltf`: a glTF file holding
//! one material and its images, which the renderer loads like any model's materials
//! (`<key>#Material0`), so scenes save it by key and it reloads when the file changes. In the
//! Content panel it shows as a ball; drag it onto an object in the scene (Unreal's Content
//! Browser, Unity's Project window), or click it to put it on the selection. One undo step.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use engine::prelude::*;
use engine::render::{MaterialRef, ResolvedMesh};
use image::{GrayImage, ImageReader, RgbaImage};

use crate::editor::{self, Editor};
use crate::undo::Command;

/// What a material file's key ends with.
pub const MATERIAL_SUFFIX: &str = ".material.gltf";
/// Folder under `imported/` that texture sets go to.
const MATERIALS_DIR: &str = "materials";

/// One map of a texture set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Map {
    BaseColor,
    /// Tangent space; `directx` when its green points down (DirectX / Unreal convention).
    Normal {
        directx: bool,
    },
    Roughness,
    /// Smoothness: 1 − roughness.
    Gloss,
    Metallic,
    Occlusion,
    /// Occlusion, roughness, metallic in R, G, B (glTF's layout; Substance's Unreal preset,
    /// Poly Haven's ARM).
    Orm,
    /// Unity HDRP: metallic, occlusion, detail mask, smoothness in R, G, B, A.
    UnityMask,
    /// Unity URP / built-in: metallic in R, smoothness in A.
    MetallicSmoothness,
    Emissive,
    Opacity,
    /// No glTF slot: noted in the summary.
    Height,
}

/// Name words for each map, longest first where one contains another. The base color, normal,
/// roughness, gloss, metallic and height words are Node Wrangler's defaults.
const TAGS: &[(&str, Map)] = &[
    ("occlusionroughnessmetallic", Map::Orm),
    ("orm", Map::Orm),
    ("arm", Map::Orm),
    ("maskmap", Map::UnityMask),
    ("metallicsmoothness", Map::MetallicSmoothness),
    ("metallicgloss", Map::MetallicSmoothness),
    ("metallicglossmap", Map::MetallicSmoothness),
    ("basecolor", Map::BaseColor),
    ("basecolour", Map::BaseColor),
    ("albedo", Map::BaseColor),
    ("diffuse", Map::BaseColor),
    ("diff", Map::BaseColor),
    ("color", Map::BaseColor),
    ("colour", Map::BaseColor),
    ("col", Map::BaseColor),
    ("normalgl", Map::Normal { directx: false }),
    ("normalopengl", Map::Normal { directx: false }),
    ("normaldx", Map::Normal { directx: true }),
    ("normaldirectx", Map::Normal { directx: true }),
    ("normal", Map::Normal { directx: false }),
    ("nor", Map::Normal { directx: false }),
    ("nrm", Map::Normal { directx: false }),
    ("nrml", Map::Normal { directx: false }),
    ("norm", Map::Normal { directx: false }),
    ("roughness", Map::Roughness),
    ("rough", Map::Roughness),
    ("rgh", Map::Roughness),
    ("glossiness", Map::Gloss),
    ("glossy", Map::Gloss),
    ("gloss", Map::Gloss),
    ("smoothness", Map::Gloss),
    ("metallic", Map::Metallic),
    ("metalness", Map::Metallic),
    ("metal", Map::Metallic),
    ("mtl", Map::Metallic),
    ("ambientocclusion", Map::Occlusion),
    ("occlusion", Map::Occlusion),
    ("ao", Map::Occlusion),
    ("emissive", Map::Emissive),
    ("emission", Map::Emissive),
    ("emit", Map::Emissive),
    ("opacity", Map::Opacity),
    ("alpha", Map::Opacity),
    ("transparency", Map::Opacity),
    ("height", Map::Height),
    ("heightmap", Map::Height),
    ("displacement", Map::Height),
    ("displace", Map::Height),
    ("disp", Map::Height),
    ("dsp", Map::Height),
];

/// Image files a texture set can be made of.
pub fn is_image(path: &Path) -> bool {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    matches!(
        ext.as_str(),
        "png" | "jpg" | "jpeg" | "tga" | "bmp" | "tif" | "tiff"
    )
}

/// Words that say nothing about the map: sizes ("2k", "4096"), formats, variants.
fn is_noise(word: &str) -> bool {
    let lower = word.to_ascii_lowercase();
    let size = lower
        .strip_suffix('k')
        .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
        || (lower.len() >= 3 && lower.chars().all(|c| c.is_ascii_digit()));
    size || matches!(
        lower.as_str(),
        "png" | "jpg" | "jpeg" | "tga" | "tif" | "tiff" | "exr" | "8bit" | "16bit" | "lod0" | "map"
    )
}

/// The set a file belongs to and its map, from the words of its name: `"Cliff_Rock_BaseColor"`
/// → `("Cliff_Rock", BaseColor)`, `"rock_face_nor_gl_2k"` → `("rock_face", Normal)`.
pub fn classify(stem: &str) -> Option<(String, Map)> {
    let words: Vec<&str> = stem
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();
    let mut directx = false;
    // The map word is the last meaningful word; "gl" / "dx" after a normal say which kind.
    let mut end = words.len();
    while end > 0 {
        let w = words[end - 1].to_ascii_lowercase();
        if is_noise(&w) {
            end -= 1;
            continue;
        }
        match w.as_str() {
            "gl" | "ogl" | "opengl" => end -= 1,
            "dx" | "directx" => {
                directx = true;
                end -= 1;
            }
            _ => break,
        }
    }
    let word = words.get(end.checked_sub(1)?)?.to_ascii_lowercase();
    let mut map = TAGS.iter().find(|(tag, _)| *tag == word)?.1;
    if let Map::Normal { directx: dx } = &mut map {
        *dx |= directx;
    }
    // "base_color", "ambient_occlusion": the first word belongs to the map too.
    let mut start = end - 1;
    if start > 0 {
        let before = words[start - 1].to_ascii_lowercase();
        if (before == "base" && map == Map::BaseColor)
            || (before == "ambient" && map == Map::Occlusion)
        {
            start -= 1;
        }
    }
    // The rest is the set's name, without trailing sizes and formats.
    let mut name_end = start;
    while name_end > 0 && is_noise(words[name_end - 1]) {
        name_end -= 1;
    }
    let name = words[..name_end].join("_");
    Some((
        if name.is_empty() {
            "material".into()
        } else {
            name
        },
        map,
    ))
}

/// A material's worth of maps.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TextureSet {
    pub name: String,
    pub maps: BTreeMap<Map, PathBuf>,
}

impl TextureSet {
    /// Enough to be a material: a color, or at least two maps of any kind.
    fn usable(&self) -> bool {
        self.maps.contains_key(&Map::BaseColor) || self.maps.len() >= 2
    }
}

/// Group image files into texture sets by name; files that fit no set are returned apart.
pub fn texture_sets(files: &[PathBuf]) -> (Vec<TextureSet>, Vec<PathBuf>) {
    let mut sets: BTreeMap<String, TextureSet> = BTreeMap::new();
    let mut loose = Vec::new();
    for file in files {
        let stem = file.file_stem().unwrap_or_default().to_string_lossy();
        match classify(&stem) {
            Some((name, map)) => {
                let set = sets
                    .entry(name.to_ascii_lowercase())
                    .or_insert_with(|| TextureSet {
                        name: name.clone(),
                        maps: BTreeMap::new(),
                    });
                set.maps.entry(map).or_insert_with(|| file.clone());
            }
            None => loose.push(file.clone()),
        }
    }
    let mut out = Vec::new();
    for set in sets.into_values() {
        if set.usable() {
            out.push(set);
        } else {
            loose.extend(set.maps.into_values());
        }
    }
    (out, loose)
}

fn open(path: &Path) -> Result<image::DynamicImage, String> {
    ImageReader::open(path)
        .and_then(|r| r.with_guessed_format())
        .map_err(|e| format!("{}: {e}", path.display()))?
        .decode()
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// One channel of `path` at `size` (red of a color image, luminance of a gray one).
fn channel(path: &Path, size: (u32, u32), pick: usize) -> Result<GrayImage, String> {
    let img = open(path)?;
    let img = if (img.width(), img.height()) == size {
        img
    } else {
        img.resize_exact(size.0, size.1, image::imageops::FilterType::Triangle)
    };
    let rgba = img.to_rgba8();
    let gray = if img.color().has_color() || pick == 3 {
        GrayImage::from_fn(size.0, size.1, |x, y| {
            image::Luma([rgba.get_pixel(x, y)[pick]])
        })
    } else {
        img.to_luma8()
    };
    Ok(gray)
}

/// Pixel size of an image without decoding it.
fn dimensions(path: &Path) -> Result<(u32, u32), String> {
    ImageReader::open(path)
        .and_then(|r| r.with_guessed_format())
        .map_err(|e| format!("{}: {e}", path.display()))?
        .into_dimensions()
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// Copy a PNG or JPEG as it is, or write anything else as PNG. Returns the file name used.
fn copy_image(from: &Path, dir: &Path, name: &str) -> Result<String, String> {
    let ext = from
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "png" | "jpg" | "jpeg" => {
            let file = format!("{name}.{ext}");
            std::fs::copy(from, dir.join(&file)).map_err(|e| format!("{}: {e}", from.display()))?;
            Ok(file)
        }
        _ => {
            let file = format!("{name}.png");
            save(&open(from)?.to_rgba8(), &dir.join(&file))?;
            Ok(file)
        }
    }
}

fn save(img: &RgbaImage, path: &Path) -> Result<(), String> {
    img.save(path)
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// What a texture set became.
#[derive(Debug, Clone, PartialEq)]
pub struct Built {
    /// Asset key of the material file.
    pub key: String,
    /// "color, normal (DirectX, flipped), roughness + occlusion packed".
    pub summary: String,
}

/// Write `set` as a material file under `root` (the asset root).
pub fn build(root: &Path, set: &TextureSet) -> Result<Built, String> {
    let slug = crate::import::slug(&set.name);
    let folder = format!("{}/{MATERIALS_DIR}/{slug}", crate::import::IMPORTED_DIR);
    let dir = root.join(&folder);
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut images: Vec<String> = Vec::new();
    let mut add = |file: String| {
        images.push(file);
        images.len() - 1
    };
    let mut notes: Vec<String> = Vec::new();
    let maps = &set.maps;

    // Color, with an opacity map in its alpha.
    let mut color = None;
    let mut cutout = false;
    if let Some(path) = maps.get(&Map::BaseColor) {
        let file = match maps.get(&Map::Opacity) {
            Some(opacity) => {
                let mut rgba = open(path)?.to_rgba8();
                let alpha = channel(opacity, rgba.dimensions(), 0)?;
                for (p, a) in rgba.pixels_mut().zip(alpha.pixels()) {
                    p[3] = a[0];
                }
                let file = format!("{slug}_basecolor.png");
                save(&rgba, &dir.join(&file))?;
                cutout = true;
                notes.push("color with opacity in alpha".into());
                file
            }
            None => {
                notes.push("color".into());
                copy_image(path, &dir, &format!("{slug}_basecolor"))?
            }
        };
        color = Some(add(file));
    }

    // Normal, flipped to OpenGL's +Y when it came in DirectX's.
    let mut normal = None;
    for directx in [false, true] {
        let Some(path) = maps.get(&Map::Normal { directx }) else {
            continue;
        };
        let file = if directx {
            let mut rgba = open(path)?.to_rgba8();
            for p in rgba.pixels_mut() {
                p[1] = 255 - p[1];
            }
            let file = format!("{slug}_normal.png");
            save(&rgba, &dir.join(&file))?;
            notes.push("normal (DirectX, green flipped)".into());
            file
        } else {
            notes.push("normal".into());
            copy_image(path, &dir, &format!("{slug}_normal"))?
        };
        normal = Some(add(file));
        break;
    }

    // Occlusion, roughness, metallic: packed as they are, or packed here.
    let mut orm = None;
    let (mut has_occlusion, mut has_roughness, mut has_metallic) = (false, false, false);
    if let Some(path) = maps.get(&Map::Orm) {
        orm = Some(add(copy_image(path, &dir, &format!("{slug}_orm"))?));
        (has_occlusion, has_roughness, has_metallic) = (true, true, true);
        notes.push("occlusion-roughness-metallic as packed".into());
    } else {
        // Sources for each channel: (file, channel, invert).
        let mut r = maps.get(&Map::Occlusion).map(|p| (p, 0, false));
        let mut g = maps
            .get(&Map::Roughness)
            .map(|p| (p, 0, false))
            .or_else(|| maps.get(&Map::Gloss).map(|p| (p, 0, true)));
        let mut b = maps.get(&Map::Metallic).map(|p| (p, 0, false));
        if let Some(p) = maps.get(&Map::UnityMask) {
            b = b.or(Some((p, 0, false)));
            r = r.or(Some((p, 1, false)));
            g = g.or(Some((p, 3, true)));
            notes.push("Unity mask map unpacked".into());
        } else if let Some(p) = maps.get(&Map::MetallicSmoothness) {
            b = b.or(Some((p, 0, false)));
            g = g.or(Some((p, 3, true)));
            notes.push("Unity metallic-smoothness unpacked".into());
        }
        if r.is_some() || g.is_some() || b.is_some() {
            let size = [r, g, b]
                .into_iter()
                .flatten()
                .map(|(p, _, _)| dimensions(p))
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .max_by_key(|(w, h)| w * h)
                .unwrap_or((1, 1));
            let read = |source: Option<(&PathBuf, usize, bool)>, fill: u8| match source {
                Some((p, c, invert)) => channel(p, size, c).map(|mut img| {
                    if invert {
                        image::imageops::invert(&mut img);
                    }
                    img
                }),
                None => Ok(GrayImage::from_pixel(size.0, size.1, image::Luma([fill]))),
            };
            let (cr, cg, cb) = (read(r, 255)?, read(g, 255)?, read(b, 0)?);
            let packed = RgbaImage::from_fn(size.0, size.1, |x, y| {
                image::Rgba([
                    cr.get_pixel(x, y)[0],
                    cg.get_pixel(x, y)[0],
                    cb.get_pixel(x, y)[0],
                    255,
                ])
            });
            let file = format!("{slug}_orm.png");
            save(&packed, &dir.join(&file))?;
            orm = Some(add(file));
            (has_occlusion, has_roughness, has_metallic) = (r.is_some(), g.is_some(), b.is_some());
            let mut parts = Vec::new();
            if g.is_some() {
                parts.push(if maps.contains_key(&Map::Gloss) {
                    "roughness (from gloss)"
                } else {
                    "roughness"
                });
            }
            if b.is_some() {
                parts.push("metallic");
            }
            if r.is_some() {
                parts.push("occlusion");
            }
            notes.push(format!("{} packed", parts.join(" + ")));
        }
    }

    let mut emissive = None;
    if let Some(path) = maps.get(&Map::Emissive) {
        emissive = Some(add(copy_image(path, &dir, &format!("{slug}_emissive"))?));
        notes.push("emissive".into());
    }
    if maps.contains_key(&Map::Height) {
        notes.push("height map left out (glTF has no slot for it)".into());
    }

    // The glTF file: one material, its images.
    let tex = |i: usize| serde_json::json!({ "index": i });
    let mut pbr = serde_json::Map::new();
    if let Some(i) = color {
        pbr.insert("baseColorTexture".into(), tex(i));
    }
    pbr.insert(
        "metallicFactor".into(),
        (if has_metallic { 1.0 } else { 0.0 }).into(),
    );
    pbr.insert(
        "roughnessFactor".into(),
        (if has_roughness { 1.0 } else { 0.6 }).into(),
    );
    if let (Some(i), true) = (orm, has_roughness || has_metallic) {
        pbr.insert("metallicRoughnessTexture".into(), tex(i));
    }
    let mut material = serde_json::Map::new();
    material.insert("name".into(), set.name.clone().into());
    material.insert("pbrMetallicRoughness".into(), pbr.into());
    if let Some(i) = normal {
        material.insert("normalTexture".into(), tex(i));
    }
    if let (Some(i), true) = (orm, has_occlusion) {
        material.insert("occlusionTexture".into(), tex(i));
    }
    if let Some(i) = emissive {
        material.insert("emissiveTexture".into(), tex(i));
        material.insert("emissiveFactor".into(), serde_json::json!([1.0, 1.0, 1.0]));
    }
    if cutout {
        material.insert("alphaMode".into(), "MASK".into());
        material.insert("alphaCutoff".into(), 0.5.into());
    }
    let doc = serde_json::json!({
        "asset": { "version": "2.0", "generator": "Engine editor (texture set import)" },
        "images": images.iter().map(|uri| serde_json::json!({ "uri": uri })).collect::<Vec<_>>(),
        "textures": (0..images.len()).map(|i| serde_json::json!({ "source": i })).collect::<Vec<_>>(),
        "materials": [material],
    });
    let file = format!("{slug}{MATERIAL_SUFFIX}");
    let text = serde_json::to_string_pretty(&doc).map_err(|e| e.to_string())?;
    std::fs::write(dir.join(&file), text).map_err(|e| format!("{}: {e}", dir.display()))?;
    Ok(Built {
        key: format!("{folder}/{file}"),
        summary: notes.join(", "),
    })
}

/// A material file (by asset key).
pub fn is_material(key: &str) -> bool {
    key.ends_with(MATERIAL_SUFFIX)
}

/// "Cliff Rock" for "imported/materials/cliff_rock/cliff_rock.material.gltf".
pub fn display_name(key: &str) -> String {
    let file = key.rsplit('/').next().unwrap_or(key);
    file.strip_suffix(MATERIAL_SUFFIX)
        .unwrap_or(file)
        .replace('_', " ")
}

/// The mesh entity a material dropped along this ray lands on.
pub fn target(world: &mut World, origin: DVec3, dir: Vec3) -> Option<Entity> {
    let (e, _) = crate::picking::pick_mesh(world, origin, dir)?;
    world.get::<MeshRenderer>(e).map(|_| e)
}

/// Put the material file `key` on `entities`' meshes, one undo step. Rigs keep theirs.
pub fn apply(world: &mut World, editor: &mut Editor, key: &str, entities: &[Entity]) -> usize {
    let material = MaterialRef::Asset(format!("{key}#Material0"));
    let mut commands = Vec::new();
    for &e in entities {
        let Some(renderer) = world.get::<MeshRenderer>(e) else {
            continue;
        };
        if renderer.material == material {
            continue;
        }
        let after = MeshRenderer {
            material: material.clone(),
            ..renderer.clone()
        };
        let (Ok(before), Ok(after)) = (serde_json::to_value(renderer), serde_json::to_value(after))
        else {
            continue;
        };
        commands.push(Command::Component {
            entity: e,
            name: "MeshRenderer".into(),
            before: Some(before),
            after: Some(after),
        });
    }
    let n = commands.len();
    let name = display_name(key);
    let t = editor::now(world);
    let command = match n {
        0 => {
            editor.info(
                t,
                format!("Nothing to put {name} on: select or aim at a mesh"),
            );
            return 0;
        }
        1 => Command::Group(
            format!(
                "Material {name} on {}",
                crate::scene_ops::label(world, entities[0])
            ),
            commands,
        ),
        _ => Command::Group(format!("Material {name} on {n} objects"), commands),
    };
    if let Err(e) = editor.history.apply(world, command, t) {
        editor.error(t, format!("Could not apply {name}: {e}"));
        return 0;
    }
    // Rebuild the resolved handles at once, so the picture changes this frame.
    for &e in entities {
        world.entity_mut(e).remove::<ResolvedMesh>();
    }
    n
}

/// The selection and its children's meshes, for a click on a material.
pub fn selected_meshes(world: &World, editor: &Editor) -> Vec<Entity> {
    let mut out = Vec::new();
    let mut stack: Vec<Entity> = editor.selection.clone();
    while let Some(e) = stack.pop() {
        if world.get::<MeshRenderer>(e).is_some() {
            out.push(e);
        }
        if let Some(children) = world.get::<Children>(e) {
            stack.extend(children.iter());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_from_common_exporters() {
        let n = |dx| Map::Normal { directx: dx };
        for (stem, name, map) in [
            ("Cliff_Rock_BaseColor", "Cliff_Rock", Map::BaseColor),
            (
                "Cliff_Rock_OcclusionRoughnessMetallic",
                "Cliff_Rock",
                Map::Orm,
            ),
            ("Cliff_Rock_Normal", "Cliff_Rock", n(false)),
            ("rock_face_diff_2k", "rock_face", Map::BaseColor),
            ("rock_face_nor_gl_2k", "rock_face", n(false)),
            ("rock_face_nor_dx_4k", "rock_face", n(true)),
            ("rock_face_arm_2k", "rock_face", Map::Orm),
            ("Ground054_2K-PNG_Color", "Ground054", Map::BaseColor),
            ("Ground054_2K-PNG_NormalDX", "Ground054", n(true)),
            (
                "Ground054_2K-PNG_AmbientOcclusion",
                "Ground054",
                Map::Occlusion,
            ),
            (
                "Mud_Wet_ukmkdc2_2K_Albedo",
                "Mud_Wet_ukmkdc2",
                Map::BaseColor,
            ),
            ("Mud_Wet_ukmkdc2_2K_Gloss", "Mud_Wet_ukmkdc2", Map::Gloss),
            ("T_Crate_base_color", "T_Crate", Map::BaseColor),
            ("crate ambient occlusion", "crate", Map::Occlusion),
            ("Robot_MaskMap", "Robot", Map::UnityMask),
            ("bricks-height", "bricks", Map::Height),
        ] {
            assert_eq!(classify(stem), Some((name.to_string(), map)), "{stem}");
        }
        assert_eq!(classify("holiday_photo"), None);
        assert_eq!(classify("4096"), None);
    }

    #[test]
    fn a_folder_with_two_sets_and_a_stray_image() {
        let files: Vec<PathBuf> = [
            "a/Bricks_Color.png",
            "a/Bricks_Normal.png",
            "a/Bricks_Roughness.png",
            "a/Wood_diff_1k.jpg",
            "a/Wood_rough_1k.jpg",
            "a/Wood_nor_gl_1k.jpg",
            "a/readme_photo.jpg",
            "a/Lonely_AO.png",
        ]
        .iter()
        .map(PathBuf::from)
        .collect();
        let (sets, loose) = texture_sets(&files);
        let names: Vec<&str> = sets.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["Bricks", "Wood"]);
        assert_eq!(sets[0].maps.len(), 3);
        assert_eq!(loose.len(), 2, "{loose:?}");
    }

    #[test]
    fn separate_maps_pack_into_gltf_layout() {
        let dir = std::env::temp_dir().join(format!("editor-materials-{}", std::process::id()));
        let src = dir.join("src");
        std::fs::create_dir_all(&src).unwrap();
        let gray = |v: u8| RgbaImage::from_pixel(4, 4, image::Rgba([v, v, v, 255]));
        save(
            &RgbaImage::from_pixel(4, 4, image::Rgba([200, 30, 30, 255])),
            &src.join("Box_Color.png"),
        )
        .unwrap();
        // A DirectX normal pointing straight out has green 128; one tilted up has green < 128.
        save(
            &RgbaImage::from_pixel(4, 4, image::Rgba([128, 40, 255, 255])),
            &src.join("Box_NormalDX.png"),
        )
        .unwrap();
        save(&gray(255 - 70), &src.join("Box_Gloss.png")).unwrap();
        save(&gray(20), &src.join("Box_Metallic.png")).unwrap();
        save(&gray(180), &src.join("Box_AO.png")).unwrap();
        let files: Vec<PathBuf> = std::fs::read_dir(&src)
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        let (sets, _) = texture_sets(&files);
        let built = build(&dir, &sets[0]).unwrap();
        assert_eq!(built.key, "imported/materials/box/box.material.gltf");
        let folder = dir.join("imported/materials/box");
        let orm = image::open(folder.join("box_orm.png")).unwrap().to_rgba8();
        assert_eq!(orm.get_pixel(1, 1).0, [180, 70, 20, 255]);
        let normal = image::open(folder.join("box_normal.png"))
            .unwrap()
            .to_rgba8();
        assert_eq!(normal.get_pixel(0, 0)[1], 255 - 40);
        // The glTF file reads back with every slot filled.
        let (doc, _, images) = gltf::import(dir.join(&built.key)).unwrap();
        assert_eq!(images.len(), 3);
        let m = doc.materials().next().unwrap();
        assert!(m.pbr_metallic_roughness().base_color_texture().is_some());
        assert!(
            m.pbr_metallic_roughness()
                .metallic_roughness_texture()
                .is_some()
        );
        assert!(m.normal_texture().is_some());
        assert!(m.occlusion_texture().is_some());
        assert_eq!(m.name(), Some("Box"));
        assert!(
            built
                .summary
                .contains("roughness (from gloss) + metallic + occlusion packed"),
            "{}",
            built.summary
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
