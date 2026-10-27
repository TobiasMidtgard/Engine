//! A model's materials find their textures by name.
//!
//! Models exported from Blender, Maya or 3ds Max for texturing in Substance Painter carry named
//! materials and no textures; Painter then writes one texture set per material, named
//! `$mesh_$textureSet_<Map>` ("StoneWell_Stone_BaseColor.png"). On import the editor looks for
//! texture sets next to the model (its folder and their subfolders, then the folder above:
//! Unity's Recursive-Up, Unreal's Under Parent) and pairs each untextured material with the
//! set named after it: the material's own name, or the model's name plus it (Unity's "Model
//! Name + Model's Material"), ignoring an Unreal `T_` or `M_` prefix and a Blender `.001`.
//! Each pair is built into a material file ([`crate::materials::build`]) and recorded in the
//! model's import settings as a remap (Unity's Remapped Materials), which every placed copy
//! uses. The Details panel lists the remaps; any material can be pointed at another one there.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use engine::prelude::*;
use engine::render::{MaterialRef, MeshRenderer};
use engine_anim::skin::SkinnedMeshRenderer;

use crate::import::{ImportSettings, settings_for};
use crate::materials::{TextureSet, is_image, texture_sets};

/// One of a model file's materials.
#[derive(Debug, Clone, PartialEq)]
pub struct FileMaterial {
    pub name: String,
    /// It has a base color texture of its own.
    pub textured: bool,
}

/// The materials a glTF or GLB file defines, in file order (`#Material{i}`).
pub fn file_materials(path: &Path) -> Vec<FileMaterial> {
    let Ok(file) = gltf::Gltf::open(path) else {
        return Vec::new();
    };
    file.materials()
        .map(|m| FileMaterial {
            name: m.name().unwrap_or_default().to_string(),
            textured: m.pbr_metallic_roughness().base_color_texture().is_some(),
        })
        .collect()
}

/// Lower-case words of a name, without what says "material" or "texture" rather than which
/// one: Unreal's `T_` / `M_` / `MI_` prefixes, `_Mat` / `_Mtl` suffixes, Blender's `.001`.
fn words(name: &str) -> Vec<String> {
    let mut words: Vec<String> = name
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_ascii_lowercase)
        .collect();
    while words.len() > 1 && matches!(words[0].as_str(), "t" | "m" | "mi" | "mat" | "material") {
        words.remove(0);
    }
    while words.len() > 1 {
        let last = words[words.len() - 1].as_str();
        let copy = last.len() == 3 && last.chars().all(|c| c.is_ascii_digit());
        if copy || matches!(last, "mat" | "mtl" | "material") {
            words.pop();
        } else {
            break;
        }
    }
    words
}

/// Whether texture set `set` was made for material `material` of the model called `model`:
/// "Stone" and "StoneWell_Stone" or "T_Stone", "M_Wood_Mat.001" and "Wood". Words are
/// compared run together, so "CrateLid" matches "crate_lid".
pub fn matches(material: &str, model: &str, set: &str) -> bool {
    let material = words(material).concat();
    if material.is_empty() {
        return false;
    }
    let set = words(set).concat();
    let model = words(model).concat();
    set == material || (!model.is_empty() && set == format!("{model}{material}"))
}

/// Image files where a model's texture sets would be: its folder (and two levels of
/// subfolders, "Textures", "Maps", "Painter export"...), then the folder above and its
/// subfolders. At most a few thousand, so a model in Downloads doesn't read the whole disk.
fn images_near(dir: &Path) -> Vec<PathBuf> {
    const MAX: usize = 4000;
    fn walk(dir: &Path, depth: usize, skip: Option<&Path>, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        let mut paths: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
        paths.sort();
        for p in paths {
            if out.len() >= MAX {
                return;
            }
            if p.is_dir() {
                if depth < 2 && Some(p.as_path()) != skip {
                    walk(&p, depth + 1, skip, out);
                }
            } else if is_image(&p) {
                out.push(p);
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, 0, None, &mut out);
    if let Some(parent) = dir.parent().filter(|p| !p.as_os_str().is_empty()) {
        walk(parent, 1, Some(dir), &mut out);
    }
    out
}

/// The texture set for each of `materials` that has none of its own, from `sets`.
pub fn pair<'a>(
    materials: &[FileMaterial],
    model: &str,
    sets: &'a [TextureSet],
) -> BTreeMap<String, &'a TextureSet> {
    let mut out = BTreeMap::new();
    for m in materials
        .iter()
        .filter(|m| !m.textured && !m.name.is_empty())
    {
        if let Some(set) = sets.iter().find(|s| matches(&m.name, model, &s.name)) {
            out.insert(m.name.clone(), set);
        }
    }
    out
}

/// Pair the untextured materials of imported model `key` (under the asset `root`) with the
/// texture sets near `near`, build them, and record them in `settings`. Materials already
/// remapped keep their remap. Returns what it did, for the import summary.
pub fn search_and_remap(
    root: &Path,
    key: &str,
    near: &Path,
    settings: &mut ImportSettings,
) -> Option<String> {
    let materials: Vec<FileMaterial> = file_materials(&root.join(key))
        .into_iter()
        .filter(|m| !settings.materials.contains_key(&m.name))
        .collect();
    if materials.iter().all(|m| m.textured) {
        return None;
    }
    let (sets, _) = texture_sets(&images_near(near));
    let model = Path::new(key)
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let mut found = Vec::new();
    for (name, set) in pair(&materials, &model, &sets) {
        match crate::materials::build(root, set) {
            Ok(built) => {
                settings.materials.insert(name.clone(), built.key);
                found.push(name);
            }
            Err(e) => engine::core::log::warn!("texture set {} for {name}: {e}", set.name),
        }
    }
    (!found.is_empty()).then(|| {
        format!(
            "{} found {} ({})",
            found.len(),
            if found.len() == 1 {
                "its textures"
            } else {
                "their textures"
            },
            found.join(", ")
        )
    })
}

/// `"imported/well/Well.glb#Material2"` → 2.
fn material_index(key: &str) -> Option<usize> {
    key.rsplit_once("#Material")?.1.parse().ok()
}

/// Draw a freshly spawned copy of model `key` (rooted at `entity`) with the materials its
/// import settings remap.
pub fn apply(world: &mut World, key: &str, entity: Entity) {
    let Some(root) = world
        .get_resource::<engine::assets::AssetServer>()
        .map(|s| s.root())
    else {
        return;
    };
    let Some(settings) = settings_for(&root, key) else {
        return;
    };
    if settings.materials.is_empty() {
        return;
    }
    let names = file_materials(&root.join(key));
    let target = |i: usize| {
        let name = &names.get(i)?.name;
        let to = settings.materials.get(name).filter(|k| !k.is_empty())?;
        Some(format!("{to}#Material0"))
    };
    // Still models: one renderer per primitive, its material by key.
    let mut stack = vec![entity];
    while let Some(e) = stack.pop() {
        if let Some(children) = world.get::<Children>(e) {
            stack.extend(children.iter());
        }
        let Some(mut renderer) = world.get_mut::<MeshRenderer>(e) else {
            continue;
        };
        let to = match &renderer.material {
            MaterialRef::Asset(k) => material_index(k).and_then(target),
            _ => None,
        };
        if let Some(to) = to {
            renderer.material = MaterialRef::Asset(to);
        }
    }
    // Rigs: surfaces in file order.
    let surfaces = world
        .get::<SkinnedMeshRenderer>(entity)
        .map_or(0, |r| r.surfaces.len());
    for i in 0..surfaces {
        let Some(to) = target(i) else {
            continue;
        };
        if let Err(e) = engine::assets::load_now(world, &to) {
            engine::core::log::warn!("material {to}: {e}");
            continue;
        }
        let material = world
            .get_resource::<Assets<StandardMaterial>>()
            .and_then(|a| a.get(a.loaded_by_key(&engine::assets::normalize_path(&to))?))
            .cloned();
        if let (Some(material), Some(mut r)) =
            (material, world.get_mut::<SkinnedMeshRenderer>(entity))
        {
            r.surfaces[i] = material;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_from_painter_unreal_and_blender() {
        // Substance Painter's $mesh_$textureSet.
        assert!(matches("Stone", "StoneWell", "StoneWell_Stone"));
        // The texture set alone, an Unreal prefix, a material prefix and suffix.
        assert!(matches("Stone", "StoneWell", "Stone"));
        assert!(matches("Wood", "Cart", "T_Wood"));
        assert!(matches("M_Wood_Mat", "Cart", "wood"));
        // Blender's duplicate numbering and camel case against snake case.
        assert!(matches("CrateLid.001", "Crate", "crate_lid"));
        // Not a near miss.
        assert!(!matches("Stone", "StoneWell", "StoneWell_Stones"));
        assert!(!matches("Wood", "StoneWell", "OtherModel_Wood"));
        assert!(!matches("", "StoneWell", "StoneWell"));
    }

    #[test]
    fn only_untextured_materials_are_paired() {
        let set = |name: &str| TextureSet {
            name: name.into(),
            ..Default::default()
        };
        let sets = [set("StoneWell_Stone"), set("StoneWell_Wood"), set("Ground")];
        let materials = [
            FileMaterial {
                name: "Stone".into(),
                textured: false,
            },
            FileMaterial {
                name: "Wood".into(),
                textured: true,
            },
            FileMaterial {
                name: "Bucket_Metal".into(),
                textured: false,
            },
        ];
        let paired = pair(&materials, "StoneWell", &sets);
        assert_eq!(paired.len(), 1);
        assert_eq!(paired["Stone"].name, "StoneWell_Stone");
    }

    #[test]
    fn indices_from_material_keys() {
        assert_eq!(material_index("imported/w/W.glb#Material12"), Some(12));
        assert_eq!(material_index("imported/w/W.glb#Mesh0"), None);
    }
}
