//! Importing files from anywhere on disk into the asset folder. A glTF or GLB model (with the
//! buffers and textures a `.gltf` names beside it) or a texture is copied under
//! `<asset root>/imported/<name>/`, and its import settings are saved beside it as
//! `<file>.import.json`. While a placed model's source file changes, the editor copies it
//! again and the placed copies rebuild (see [`crate::models::watch_files`]).

use std::path::{Component, Path, PathBuf};

use engine::assets::AssetServer;
use engine::prelude::*;

use crate::editor::{Editor, now};

/// Folder under the asset root that imports go to.
pub const IMPORTED_DIR: &str = "imported";

/// Saved beside an imported file as `<file>.import.json`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ImportSettings {
    /// The file it was imported from (absolute).
    pub source: String,
    /// "model" or "texture".
    pub kind: String,
}

/// What a file imports as, from its extension.
fn kind_of(path: &Path) -> Result<&'static str, String> {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "glb" | "gltf" => Ok("model"),
        "png" | "jpg" | "jpeg" | "hdr" => Ok("texture"),
        "fbx" | "obj" | "blend" | "dae" | "3ds" | "max" | "ma" | "mb" => Err(format!(
            ".{ext} files can't be imported yet: export the model as glTF (.glb) from your \
             3D tool (Blender: File > Export > glTF 2.0)"
        )),
        _ => Err(format!(
            "don't know how to import .{ext} files (models: .glb, .gltf; textures: .png, \
             .jpg, .hdr)"
        )),
    }
}

/// "My Tree (v2)" → "my_tree_v2".
fn slug(name: &str) -> String {
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
    let key = format!("{IMPORTED_DIR}/{folder}/{file}");
    let dest = root.join(&key);
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
    let settings = ImportSettings {
        source: std::path::absolute(source)
            .unwrap_or_else(|_| source.to_path_buf())
            .to_string_lossy()
            .into_owned(),
        kind: kind.into(),
    };
    let json = serde_json::to_string_pretty(&settings).map_err(|e| e.to_string())?;
    std::fs::write(settings_path(&dest), json).map_err(|e| e.to_string())?;
    Ok(key)
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

/// Import `source` and, for a model, place it at `at` (default: where the view is looking).
pub fn import_and_place(world: &mut World, editor: &mut Editor, source: &Path, at: Option<DVec3>) {
    let t = now(world);
    let Some(root) = world.get_resource::<AssetServer>().map(|s| s.root()) else {
        return;
    };
    match import_file(&root, source) {
        Ok(key) => {
            editor.models = None;
            editor.info(t, format!("Imported {} as {key}", source.display()));
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
        assert!(kind_of(Path::new("a.FBX")).unwrap_err().contains("glTF"));
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

        // A buffer outside the folder is refused.
        let doc = serde_json::json!({"buffers": [{"uri": "../x.bin"}]});
        std::fs::write(src.join("bad.gltf"), doc.to_string()).unwrap();
        assert!(import_file(&root, &src.join("bad.gltf")).is_err());
        std::fs::remove_dir_all(&src).unwrap();
        std::fs::remove_dir_all(&root).unwrap();
    }
}
