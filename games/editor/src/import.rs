//! Importing files from anywhere on disk into the asset folder. A glTF or GLB model (with the
//! buffers and textures a `.gltf` names beside it) or a texture is copied under
//! `<asset root>/imported/<name>/`, and its import settings are saved beside it as
//! `<file>.import.json`. While a placed model's source file changes, the editor copies it
//! again and the placed copies rebuild (see [`crate::models::watch_files`]).

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use engine::assets::AssetServer;
use engine::input::FileDrop;
use engine::prelude::*;

use crate::editor::{Editor, now};

/// Folder under the asset root that imports go to.
pub const IMPORTED_DIR: &str = "imported";

/// Saved beside an imported file as `<file>.import.json` (like Godot's `.import` and Unity's
/// `.meta`): where it came from and how to bring it in. Edited in the Details panel of a
/// placed model; changes apply on Reimport.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ImportSettings {
    /// The file it was imported from (absolute).
    pub source: String,
    /// "model" or "texture".
    pub kind: String,
    /// Uniform scale applied to the model's nodes (0.01 for a file authored in centimetres
    /// that says it's in metres).
    pub scale: f64,
    /// What Make solid / Make physical fit around it: "box" (default) or "none".
    pub collider: String,
    /// What the last import found (nodes, meshes, materials, what was left out).
    pub summary: String,
    /// The model's materials by name, and the material file each is drawn with instead
    /// (Unity's Remapped Materials): texture sets found by name on import, or picked in the
    /// Details panel. An empty value draws the file's own.
    pub materials: BTreeMap<String, String>,
}

impl Default for ImportSettings {
    fn default() -> Self {
        Self {
            source: String::new(),
            kind: String::new(),
            scale: 1.0,
            collider: "box".into(),
            summary: String::new(),
            materials: BTreeMap::new(),
        }
    }
}

/// The import settings saved beside asset `key`, if it was imported.
pub fn settings_for(root: &Path, key: &str) -> Option<ImportSettings> {
    let text = std::fs::read_to_string(settings_path(&root.join(key))).ok()?;
    serde_json::from_str(&text).ok()
}

/// Save changed settings beside asset `key`.
pub fn save_settings(root: &Path, key: &str, settings: &ImportSettings) -> Result<(), String> {
    let json = serde_json::to_string_pretty(settings).map_err(|e| e.to_string())?;
    std::fs::write(settings_path(&root.join(key)), json).map_err(|e| e.to_string())
}

/// What a file imports as, from its extension.
fn kind_of(path: &Path) -> Result<&'static str, String> {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "glb" | "gltf" | "fbx" | "obj" | "blend" => Ok("model"),
        "png" | "jpg" | "jpeg" | "hdr" => Ok("texture"),
        "dae" | "3ds" | "max" | "ma" | "mb" | "c4d" | "usd" | "usdz" => Err(format!(
            ".{ext} files can't be imported yet: export the model as glTF (.glb) or FBX from \
             your 3D tool"
        )),
        _ => Err(format!(
            "don't know how to import .{ext} files (models: .glb, .gltf, .fbx, .obj, .blend; \
             textures: .png, .jpg, .hdr)"
        )),
    }
}

/// "My Tree (v2)" → "my_tree_v2".
pub fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.trim().to_lowercase().chars() {
        if c.is_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('_') {
            out.push('_');
        }
    }
    let out = out.trim_matches('_').to_string();
    if out.is_empty() { "import".into() } else { out }
}

/// A glTF URI as a relative path inside the model's folder ("tex/Bark%20A.png" →
/// "tex/Bark A.png"); `None` for embedded data or paths that leave the folder.
fn relative_uri(uri: &str) -> Option<PathBuf> {
    if uri.starts_with("data:") || uri.contains("://") {
        return None;
    }
    let mut decoded = Vec::new();
    let bytes = uri.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(byte) = uri
                .get(i + 1..i + 3)
                .and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            decoded.push(byte);
            i += 3;
            continue;
        }
        decoded.push(bytes[i]);
        i += 1;
    }
    let path = PathBuf::from(String::from_utf8_lossy(&decoded).into_owned());
    path.components()
        .all(|c| matches!(c, Component::Normal(_)))
        .then_some(path)
}

/// The files a `.gltf` reads beside itself (buffers and images), as relative paths.
fn gltf_dependencies(gltf: &Path) -> Result<Vec<PathBuf>, String> {
    let text = std::fs::read_to_string(gltf).map_err(|e| format!("{}: {e}", gltf.display()))?;
    let doc: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("{} is not glTF: {e}", gltf.display()))?;
    let mut out = Vec::new();
    for list in ["buffers", "images"] {
        for item in doc[list].as_array().into_iter().flatten() {
            let Some(uri) = item["uri"].as_str() else {
                continue;
            };
            if uri.starts_with("data:") {
                continue;
            }
            let path = relative_uri(uri)
                .ok_or_else(|| format!("{uri} is outside the model's folder; move it beside it"))?;
            out.push(path);
        }
    }
    Ok(out)
}

/// Copy `source` into `<root>/imported/...` and return its asset key.
pub fn import_file(root: &Path, source: &Path) -> Result<String, String> {
    let kind = kind_of(source)?;
    let file = source
        .file_name()
        .ok_or("no file name")?
        .to_string_lossy()
        .into_owned();
    let folder = match kind {
        "model" => slug(&source.file_stem().unwrap_or_default().to_string_lossy()),
        _ => "textures".into(),
    };
    let ext = source
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    // Other formats become a .glb beside where the copy would go (Godot converts on import too).
    let converted = crate::convert::converts(&ext);
    let file = if converted {
        format!(
            "{}.glb",
            source.file_stem().unwrap_or_default().to_string_lossy()
        )
    } else {
        file
    };
    let key = format!("{IMPORTED_DIR}/{folder}/{file}");
    let dest = root.join(&key);
    let mut settings = settings_for(root, &key).unwrap_or_default();
    settings.source = std::path::absolute(source)
        .unwrap_or_else(|_| source.to_path_buf())
        .to_string_lossy()
        .into_owned();
    settings.kind = kind.into();
    if converted {
        if !source.is_file() {
            return Err(format!("{} is missing", source.display()));
        }
        settings.summary = crate::convert::to_glb(source, &dest)?.describe();
        find_textures(root, &key, source, &mut settings);
        save_settings(root, &key, &settings)?;
        return Ok(key);
    }
    let mut copies = vec![(source.to_path_buf(), dest.clone())];
    if file.to_ascii_lowercase().ends_with(".gltf") {
        let from = source.parent().unwrap_or(Path::new("."));
        let to = dest.parent().unwrap_or(Path::new("."));
        for dep in gltf_dependencies(source)? {
            copies.push((from.join(&dep), to.join(&dep)));
        }
    }
    for (from, _) in &copies {
        if !from.is_file() {
            return Err(format!("{} is missing", from.display()));
        }
    }
    for (from, to) in &copies {
        if let Some(dir) = to.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        std::fs::copy(from, to).map_err(|e| format!("{}: {e}", from.display()))?;
    }
    settings.summary.clear();
    find_textures(root, &key, source, &mut settings);
    save_settings(root, &key, &settings)?;
    Ok(key)
}

/// Texture sets near `source` for the imported model's untextured materials, noted in the
/// summary ([`crate::remap`]).
fn find_textures(root: &Path, key: &str, source: &Path, settings: &mut ImportSettings) {
    let near = std::path::absolute(source)
        .unwrap_or_else(|_| source.to_path_buf())
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default();
    if let Some(found) = crate::remap::search_and_remap(root, key, &near, settings) {
        if !settings.summary.is_empty() {
            settings.summary.push_str("; ");
        }
        settings.summary.push_str(&found);
    }
}

fn settings_path(asset: &Path) -> PathBuf {
    let mut name = asset.as_os_str().to_owned();
    name.push(".import.json");
    PathBuf::from(name)
}

/// If `key` was imported and its source changed since, copy it in again. True when it did.
pub fn refresh_from_source(root: &Path, key: &str) -> Result<bool, String> {
    let asset = root.join(key);
    let Ok(text) = std::fs::read_to_string(settings_path(&asset)) else {
        return Ok(false);
    };
    let settings: ImportSettings = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let modified = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    let source = Path::new(&settings.source);
    match (modified(source), modified(&asset)) {
        (Some(src), Some(copy)) if src > copy => {
            import_file(root, source)?;
            Ok(true)
        }
        (Some(_), None) => {
            import_file(root, source)?;
            Ok(true)
        }
        _ => Ok(false),
    }
}

/// The Details panel's import settings for a placed model that was imported (Godot's Import
/// dock, Unity's importer Inspector): where it came from, its scale and collider, what the
/// import found, and Apply and reimport.
pub fn settings_panel(ui: &mut egui::Ui, world: &mut World, editor: &mut Editor, entity: Entity) {
    let Some(key) = world
        .get::<crate::models::Model>(entity)
        .map(|m| m.path.clone())
    else {
        return;
    };
    let Some(root) = world.get_resource::<AssetServer>().map(|s| s.root()) else {
        return;
    };
    if editor.import_edit.as_ref().is_none_or(|(k, _)| *k != key) {
        editor.import_edit = settings_for(&root, &key).map(|s| (key.clone(), s));
        editor.import_materials = crate::remap::file_materials(&root.join(&key));
    }
    let library: Vec<String> = crate::models::cached(world, editor)
        .iter()
        .filter(|k| crate::materials::is_material(k))
        .cloned()
        .collect();
    let file_materials = std::mem::take(&mut editor.import_materials);
    let Some((_, settings)) = editor.import_edit.as_mut() else {
        return;
    };
    let mut apply = false;
    let mut remap = false;
    let mut search = false;
    egui::CollapsingHeader::new(egui::RichText::new("📥 Import settings").strong())
        .default_open(true)
        .show(ui, |ui| {
            egui::Grid::new("import settings")
                .num_columns(2)
                .spacing([10.0, 6.0])
                .show(ui, |ui| {
                    ui.label("Source");
                    let file = Path::new(&settings.source)
                        .file_name()
                        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
                    ui.label(file).on_hover_text(&settings.source);
                    ui.end_row();
                    ui.label("Scale");
                    ui.add(
                        egui::DragValue::new(&mut settings.scale)
                            .speed(0.01)
                            .range(0.0001..=1000.0),
                    )
                    .on_hover_text("0.01 for a file in centimetres that says metres");
                    ui.end_row();
                    ui.label("Collider");
                    egui::ComboBox::from_id_salt("import collider")
                        .selected_text(settings.collider.as_str())
                        .show_ui(ui, |ui| {
                            for c in ["box", "none"] {
                                ui.selectable_value(&mut settings.collider, c.to_string(), c);
                            }
                        });
                    ui.end_row();
                });
            if !settings.summary.is_empty() {
                ui.label(crate::theme::dim(&settings.summary));
            }
            if !file_materials.is_empty() {
                remap = materials_list(ui, settings, &file_materials, &library);
                search = ui
                    .button("🔍 Search and remap")
                    .on_hover_text(
                        "Look beside the source file for texture sets named after the \
                         materials that have none (Substance Painter's mesh_material_BaseColor)",
                    )
                    .clicked();
            }
            apply = ui
                .button("🔄 Apply and reimport")
                .on_hover_text(
                    "Save these settings, import the source again and rebuild every copy",
                )
                .clicked();
        });
    editor.import_materials = file_materials;
    let edited = settings.clone();
    if search {
        search_textures(world, editor, &key, edited);
        return;
    }
    if remap {
        save_remaps(world, editor, &key, &edited);
        return;
    }
    if apply {
        let t = now(world);
        if let Err(e) = save_settings(&root, &key, &edited) {
            editor.error(t, format!("Could not save import settings: {e}"));
            return;
        }
        let source = Path::new(&edited.source);
        if source.is_file()
            && let Err(e) = import_file(&root, source)
        {
            editor.error(t, format!("Could not import {}: {e}", source.display()));
        }
        editor.model_info.remove(&key);
        editor.import_edit = None;
        crate::models::reimport(world, editor, &key);
    }
}

/// Look beside model `key`'s source for texture sets named after its untextured materials
/// (the Details panel's Search and remap), starting from `settings`, and rebuild its copies.
pub fn search_textures(
    world: &mut World,
    editor: &mut Editor,
    key: &str,
    mut settings: ImportSettings,
) {
    let t = now(world);
    let Some(root) = world.get_resource::<AssetServer>().map(|s| s.root()) else {
        return;
    };
    // Beside the source file, or beside the copy when the source is gone.
    let near = Path::new(&settings.source)
        .parent()
        .filter(|p| p.is_dir())
        .map(Path::to_path_buf)
        .or_else(|| root.join(key).parent().map(Path::to_path_buf))
        .unwrap_or_default();
    match crate::remap::search_and_remap(&root, key, &near, &mut settings) {
        Some(found) => {
            editor.info(t, format!("{}: {found}", crate::models::display_name(key)));
            editor.models = None;
            save_remaps(world, editor, key, &settings);
        }
        None => editor.info(
            t,
            "No texture sets named after its untextured materials were found",
        ),
    }
}

/// Save model `key`'s `settings` with changed material remaps and rebuild its copies.
pub fn save_remaps(world: &mut World, editor: &mut Editor, key: &str, settings: &ImportSettings) {
    let t = now(world);
    let Some(root) = world.get_resource::<AssetServer>().map(|s| s.root()) else {
        return;
    };
    if let Err(e) = save_settings(&root, key, settings) {
        editor.error(t, format!("Could not save import settings: {e}"));
        return;
    }
    editor.import_edit = None;
    crate::models::reimport(world, editor, key);
}

/// The model's materials and what each is drawn with (Unity's Remapped Materials). True when
/// one was changed.
fn materials_list(
    ui: &mut egui::Ui,
    settings: &mut ImportSettings,
    file_materials: &[crate::remap::FileMaterial],
    library: &[String],
) -> bool {
    let mut changed = false;
    ui.add_space(4.0);
    ui.label(egui::RichText::new("Materials").strong());
    egui::Grid::new("import materials")
        .num_columns(2)
        .spacing([10.0, 4.0])
        .show(ui, |ui| {
            for (i, m) in file_materials.iter().enumerate() {
                let name = if m.name.is_empty() {
                    format!("Material {i}")
                } else {
                    m.name.clone()
                };
                ui.label(&name);
                let current = settings.materials.get(&m.name).cloned().unwrap_or_default();
                let shown = if current.is_empty() {
                    crate::theme::dim(if m.textured {
                        "From file"
                    } else {
                        "From file (untextured)"
                    })
                } else {
                    egui::RichText::new(crate::materials::display_name(&current))
                };
                let mut picked = current.clone();
                egui::ComboBox::from_id_salt(("remap", i))
                    .selected_text(shown)
                    .width(170.0)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut picked, String::new(), "From file");
                        for key in library {
                            ui.selectable_value(
                                &mut picked,
                                key.clone(),
                                crate::materials::display_name(key),
                            );
                        }
                    })
                    .response
                    .on_hover_text(if current.is_empty() {
                        "The file's own material".to_string()
                    } else {
                        current.clone()
                    });
                if picked != current && !m.name.is_empty() {
                    if picked.is_empty() {
                        settings.materials.remove(&m.name);
                    } else {
                        settings.materials.insert(m.name.clone(), picked);
                    }
                    changed = true;
                }
                ui.end_row();
            }
        });
    changed
}

/// Files dragged from the desktop onto the window (Update, before the UI).
pub fn file_drops(mut drops: MessageReader<FileDrop>, mut editor: ResMut<Editor>) {
    for drop in drops.read() {
        match drop {
            FileDrop::Hovered(path) => editor.file_hover = Some(path.clone()),
            FileDrop::Dropped(path) => {
                editor.file_hover = None;
                editor.file_drops.push(path.clone());
            }
            FileDrop::Cancelled => editor.file_hover = None,
        }
    }
}

/// Import files dropped anywhere but the scene view where the view is looking, side by side;
/// a texture set dropped on a mesh (`onto`) goes onto it.
pub fn import_dropped(
    world: &mut World,
    editor: &mut Editor,
    at: Option<DVec3>,
    onto: Option<Entity>,
) {
    let drops = std::mem::take(&mut editor.file_drops);
    import_paths(world, editor, &drops, at, onto);
}

/// Every file under `path` (itself when it is a file), a few folders deep, sorted.
fn files_under(path: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        let mut paths: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
        paths.sort();
        for p in paths {
            if p.is_dir() {
                if depth < 4 {
                    walk(&p, depth + 1, out);
                }
            } else {
                out.push(p);
            }
        }
    }
    if path.is_dir() {
        let mut out = Vec::new();
        walk(path, 0, &mut out);
        out
    } else {
        vec![path.to_path_buf()]
    }
}

/// Import files and folders: images that make texture sets become materials (put on `onto`
/// when there is one material), models are placed side by side from `at`, other images are
/// copied as textures. Unknown files in a folder are skipped quietly.
pub fn import_paths(
    world: &mut World,
    editor: &mut Editor,
    paths: &[PathBuf],
    at: Option<DVec3>,
    onto: Option<Entity>,
) {
    let t = now(world);
    let Some(root) = world.get_resource::<AssetServer>().map(|s| s.root()) else {
        return;
    };
    let mut images = Vec::new();
    let mut others = Vec::new();
    for path in paths {
        let from_folder = path.is_dir();
        for file in files_under(path) {
            if crate::materials::is_image(&file) {
                images.push(file);
            } else if !from_folder || kind_of(&file).is_ok() {
                others.push(file);
            }
        }
    }
    let (sets, loose) = crate::materials::texture_sets(&images);
    let mut built = Vec::new();
    for set in &sets {
        match crate::materials::build(&root, set) {
            Ok(b) => {
                let source = set
                    .maps
                    .values()
                    .next()
                    .and_then(|p| p.parent())
                    .map(|p| std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf()))
                    .unwrap_or_default();
                let settings = ImportSettings {
                    source: source.to_string_lossy().into_owned(),
                    kind: "material".into(),
                    summary: b.summary.clone(),
                    ..Default::default()
                };
                save_settings(&root, &b.key, &settings).ok();
                editor.info(
                    t,
                    format!(
                        "Material {} from {} maps: {}",
                        crate::materials::display_name(&b.key),
                        set.maps.len(),
                        b.summary
                    ),
                );
                built.push(b.key);
            }
            Err(e) => editor.error(t, format!("Could not make material {}: {e}", set.name)),
        }
    }
    if !built.is_empty() {
        editor.models = None;
    }
    if let ([key], Some(target)) = (built.as_slice(), onto) {
        crate::materials::apply(world, editor, key, &[target]);
    }
    let base = at.unwrap_or_else(|| crate::editor::placement_point(world, editor));
    for (i, path) in loose.iter().chain(&others).enumerate() {
        import_and_place(world, editor, path, Some(base + DVec3::X * 2.0 * i as f64));
    }
}

/// Import `source` and, for a model, place it at `at` (default: where the view is looking).
pub fn import_and_place(world: &mut World, editor: &mut Editor, source: &Path, at: Option<DVec3>) {
    if source.is_dir() {
        import_paths(world, editor, &[source.to_path_buf()], at, None);
        return;
    }
    let t = now(world);
    let Some(root) = world.get_resource::<AssetServer>().map(|s| s.root()) else {
        return;
    };
    match import_file(&root, source) {
        Ok(key) => {
            editor.models = None;
            let summary = settings_for(&root, &key)
                .map(|s| s.summary)
                .filter(|s| !s.is_empty())
                .map_or(String::new(), |s| format!(": {s}"));
            editor.info(
                t,
                format!("Imported {} as {key}{summary}", source.display()),
            );
            if key.ends_with(".glb") || key.ends_with(".gltf") {
                let at = at.unwrap_or_else(|| crate::editor::placement_point(world, editor));
                crate::models::place(world, editor, &key, at);
            }
        }
        Err(e) => editor.error(t, format!("Could not import {}: {e}", source.display())),
    }
}

#[cfg(test)]
mod tests {
    use std::time::SystemTime;

    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("editor-import-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn names_and_uris() {
        assert_eq!(slug("My Tree (v2)"), "my_tree_v2");
        assert_eq!(slug("  "), "import");
        assert_eq!(
            relative_uri("tex/Bark%20A.png"),
            Some(PathBuf::from("tex/Bark A.png"))
        );
        assert_eq!(relative_uri("../secret.bin"), None);
        assert_eq!(relative_uri("/etc/passwd"), None);
        assert_eq!(
            relative_uri("data:application/octet-stream;base64,AAAA"),
            None
        );
        assert!(kind_of(Path::new("a.c4d")).unwrap_err().contains("glTF"));
        assert_eq!(kind_of(Path::new("a.FBX")), Ok("model"));
        assert_eq!(kind_of(Path::new("a.GLB")), Ok("model"));
    }

    #[test]
    fn gltf_brings_its_buffers_and_textures() {
        let src = temp("src");
        let root = temp("root");
        std::fs::create_dir_all(src.join("tex")).unwrap();
        std::fs::write(src.join("rock.bin"), [0u8; 4]).unwrap();
        std::fs::write(src.join("tex/rock albedo.png"), [1u8; 4]).unwrap();
        let doc = serde_json::json!({
            "asset": {"version": "2.0"},
            "buffers": [{"uri": "rock.bin", "byteLength": 4}],
            "images": [{"uri": "tex/rock%20albedo.png"}, {"bufferView": 0}]
        });
        std::fs::write(src.join("Big Rock.gltf"), doc.to_string()).unwrap();

        let key = import_file(&root, &src.join("Big Rock.gltf")).unwrap();
        assert_eq!(key, "imported/big_rock/Big Rock.gltf");
        let dir = root.join("imported/big_rock");
        assert!(dir.join("rock.bin").is_file());
        assert!(dir.join("tex/rock albedo.png").is_file());
        let settings: ImportSettings = serde_json::from_str(
            &std::fs::read_to_string(dir.join("Big Rock.gltf.import.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(settings.kind, "model");
        assert!(settings.source.ends_with("Big Rock.gltf"));

        // Nothing changed: no copy. The source changes: copied again.
        assert_eq!(refresh_from_source(&root, &key), Ok(false));
        let later = SystemTime::now() + std::time::Duration::from_secs(5);
        std::fs::File::options()
            .append(true)
            .open(src.join("Big Rock.gltf"))
            .unwrap()
            .set_modified(later)
            .unwrap();
        assert_eq!(refresh_from_source(&root, &key), Ok(true));

        // An OBJ converts to a .glb; edited settings survive importing it again.
        std::fs::write(
            src.join("crate.obj"),
            "v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n",
        )
        .unwrap();
        let key = import_file(&root, &src.join("crate.obj")).unwrap();
        assert_eq!(key, "imported/crate/crate.glb");
        let mut settings = settings_for(&root, &key).unwrap();
        assert!(
            settings.summary.contains("1 triangles"),
            "{}",
            settings.summary
        );
        settings.scale = 0.01;
        save_settings(&root, &key, &settings).unwrap();
        import_file(&root, &src.join("crate.obj")).unwrap();
        assert_eq!(settings_for(&root, &key).unwrap().scale, 0.01);

        // A buffer outside the folder is refused.
        let doc = serde_json::json!({"buffers": [{"uri": "../x.bin"}]});
        std::fs::write(src.join("bad.gltf"), doc.to_string()).unwrap();
        assert!(import_file(&root, &src.join("bad.gltf")).is_err());
        std::fs::remove_dir_all(&src).unwrap();
        std::fs::remove_dir_all(&root).unwrap();
    }
}
