//! Editing materials in the Details panel.
//!
//! Selecting an object shows a section for each material file it is drawn with (Unity's
//! material Inspector under the Mesh Renderer, Unreal's Material Instance parameters): tint,
//! roughness and metalness (multipliers of their maps when the material has them), normal and
//! occlusion strength, glow, tiling, cutout or see-through, two-sided. Changes show at once on
//! every object that uses the material, a drag is one undo step, and the file is written once
//! the edit settles (a material file is plain glTF, so it stays usable anywhere).
//!
//! A material that lives inside a model file can't be edited there: "Make editable" copies it
//! and its images out into a material file of its own and remaps the model to it (Unity's
//! Extract Materials), after which it edits like any other.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::{Duration, Instant};

use engine::assets::AssetServer;
use engine::prelude::*;
use engine::render::gltf_loader::GLTF_EMISSIVE_LUMINANCE;
use engine::render::{AlphaMode, MaterialRef, MeshRenderer};
use serde_json::{Value, json};

use crate::editor::{self, Editor};
use crate::undo::Command;

/// What the Details panel edits of a material: the part of a glTF material the engine draws.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Params {
    /// Linear RGBA, multiplied with the color map.
    pub tint: [f32; 4],
    pub roughness: f32,
    pub metallic: f32,
    pub normal_strength: f32,
    pub occlusion_strength: f32,
    /// Linear RGB glow, and how bright (glTF's emissive strength: 1 is about a lit screen).
    pub glow: [f32; 3],
    pub glow_strength: f32,
    /// Texture repeats across the surface (KHR_texture_transform scale).
    pub tiling: [f32; 2],
    /// "opaque", "cutout" or "blend".
    pub surface: String,
    pub cutoff: f32,
    pub two_sided: bool,
}

impl Default for Params {
    fn default() -> Self {
        Self {
            tint: [1.0; 4],
            roughness: 1.0,
            metallic: 1.0,
            normal_strength: 1.0,
            occlusion_strength: 1.0,
            glow: [0.0; 3],
            glow_strength: 1.0,
            tiling: [1.0, 1.0],
            surface: "opaque".into(),
            cutoff: 0.5,
            two_sided: false,
        }
    }
}

/// Which maps a material has, for the labels ("× map").
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Maps {
    pub color: bool,
    pub orm: bool,
    pub normal: bool,
    pub occlusion: bool,
    pub glow: bool,
}

fn f(v: &Value, default: f32) -> f32 {
    v.as_f64().map_or(default, |x| x as f32)
}

fn array<const N: usize>(v: &Value, default: [f32; N]) -> [f32; N] {
    let mut out = default;
    if let Some(a) = v.as_array() {
        for (o, x) in out.iter_mut().zip(a) {
            *o = f(x, *o);
        }
    }
    out
}

/// The texture infos of a glTF material JSON (color, metal-rough, normal, occlusion, glow).
fn texture_infos(material: &mut Value) -> Vec<&mut Value> {
    let mut out = Vec::new();
    let Some(m) = material.as_object_mut() else {
        return out;
    };
    for (k, v) in m.iter_mut() {
        match k.as_str() {
            "normalTexture" | "occlusionTexture" | "emissiveTexture" => out.push(v),
            "pbrMetallicRoughness" => {
                if let Some(pbr) = v.as_object_mut() {
                    for (k, v) in pbr.iter_mut() {
                        if k == "baseColorTexture" || k == "metallicRoughnessTexture" {
                            out.push(v);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// A glTF material JSON's parameters and maps.
pub fn from_json(m: &Value) -> (Params, Maps) {
    let pbr = &m["pbrMetallicRoughness"];
    let mut material = m.clone();
    let tiling = texture_infos(&mut material)
        .into_iter()
        .find_map(|t| {
            let s = &t["extensions"]["KHR_texture_transform"]["scale"];
            s.is_array().then(|| array(s, [1.0, 1.0]))
        })
        .unwrap_or([1.0, 1.0]);
    let params = Params {
        tint: array(&pbr["baseColorFactor"], [1.0; 4]),
        roughness: f(&pbr["roughnessFactor"], 1.0),
        metallic: f(&pbr["metallicFactor"], 1.0),
        normal_strength: f(&m["normalTexture"]["scale"], 1.0),
        occlusion_strength: f(&m["occlusionTexture"]["strength"], 1.0),
        glow: array(&m["emissiveFactor"], [0.0; 3]),
        glow_strength: f(
            &m["extensions"]["KHR_materials_emissive_strength"]["emissiveStrength"],
            1.0,
        ),
        tiling,
        surface: match m["alphaMode"].as_str() {
            Some("MASK") => "cutout",
            Some("BLEND") => "blend",
            _ => "opaque",
        }
        .into(),
        cutoff: f(&m["alphaCutoff"], 0.5),
        two_sided: m["doubleSided"].as_bool().unwrap_or(false),
    };
    let has = |v: &Value| v.is_object();
    let maps = Maps {
        color: has(&pbr["baseColorTexture"]),
        orm: has(&pbr["metallicRoughnessTexture"]),
        normal: has(&m["normalTexture"]),
        occlusion: has(&m["occlusionTexture"]),
        glow: has(&m["emissiveTexture"]),
    };
    (params, maps)
}

/// Write `p` into a glTF material JSON, keeping its textures and anything else it has.
pub fn to_json(m: &mut Value, p: &Params) {
    if !m.is_object() {
        *m = json!({});
    }
    let pbr = &mut m["pbrMetallicRoughness"];
    if !pbr.is_object() {
        *pbr = json!({});
    }
    pbr["baseColorFactor"] = json!(p.tint);
    pbr["roughnessFactor"] = json!(p.roughness);
    pbr["metallicFactor"] = json!(p.metallic);
    if m["normalTexture"].is_object() {
        m["normalTexture"]["scale"] = json!(p.normal_strength);
    }
    if m["occlusionTexture"].is_object() {
        m["occlusionTexture"]["strength"] = json!(p.occlusion_strength);
    }
    m["emissiveFactor"] = json!(p.glow);
    if (p.glow_strength - 1.0).abs() > 1e-6 {
        m["extensions"]["KHR_materials_emissive_strength"] =
            json!({ "emissiveStrength": p.glow_strength });
    } else if let Some(ext) = m["extensions"].as_object_mut() {
        ext.remove("KHR_materials_emissive_strength");
    }
    let tiled = p.tiling != [1.0, 1.0];
    for info in texture_infos(m) {
        if tiled {
            info["extensions"]["KHR_texture_transform"] = json!({ "scale": p.tiling });
        } else if let Some(ext) = info["extensions"].as_object_mut() {
            ext.remove("KHR_texture_transform");
        }
    }
    let obj = m.as_object_mut().expect("an object");
    match p.surface.as_str() {
        "cutout" => {
            obj.insert("alphaMode".into(), "MASK".into());
            obj.insert("alphaCutoff".into(), json!(p.cutoff));
        }
        "blend" => {
            obj.insert("alphaMode".into(), "BLEND".into());
            obj.remove("alphaCutoff");
        }
        _ => {
            obj.remove("alphaMode");
            obj.remove("alphaCutoff");
        }
    }
    if p.two_sided {
        obj.insert("doubleSided".into(), true.into());
    } else {
        obj.remove("doubleSided");
    }
    if obj
        .get("extensions")
        .is_some_and(|e| e.as_object().is_some_and(|e| e.is_empty()))
    {
        obj.remove("extensions");
    }
}

/// List the extensions a document's materials use.
fn note_extensions(doc: &mut Value) {
    let mut used = Vec::new();
    let text = doc["materials"].to_string();
    for ext in ["KHR_texture_transform", "KHR_materials_emissive_strength"] {
        if text.contains(ext) {
            used.push(ext);
        }
    }
    if used.is_empty() {
        if let Some(obj) = doc.as_object_mut() {
            obj.remove("extensionsUsed");
        }
    } else {
        doc["extensionsUsed"] = json!(used);
    }
}

fn read_doc(path: &Path) -> Result<Value, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
}

/// The parameters and maps of material file `key`.
pub fn read(root: &Path, key: &str) -> Result<(Params, Maps), String> {
    let doc = read_doc(&root.join(key))?;
    Ok(from_json(&doc["materials"][0]))
}

/// Write `p` into material file `key`.
pub fn save(root: &Path, key: &str, p: &Params) -> Result<(), String> {
    let path = root.join(key);
    let mut doc = read_doc(&path)?;
    to_json(&mut doc["materials"][0], p);
    note_extensions(&mut doc);
    let text = serde_json::to_string_pretty(&doc).map_err(|e| e.to_string())?;
    std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))
}

/// Material files waiting to be written, and when they were last changed.
#[derive(Resource, Default)]
pub struct Pending(BTreeMap<String, (Params, Instant)>);

/// Seconds an edit has to rest before it is written (a file write reloads the material).
const SETTLE: Duration = Duration::from_millis(600);

/// Show `p` on material file `key` at once (every object drawn with it) and write the file
/// once the edit settles.
pub fn set(world: &mut World, key: &str, p: &Params) {
    let asset = engine::assets::normalize_path(&format!("{key}#Material0"));
    if let Some(mut store) = world.get_resource_mut::<Assets<StandardMaterial>>()
        && let Some(handle) = store.loaded_by_key(&asset)
        && let Some(m) = store.get_mut(handle)
    {
        m.base_color = p.tint;
        m.perceptual_roughness = p.roughness;
        m.metallic = p.metallic;
        m.normal_scale = p.normal_strength;
        m.occlusion_strength = p.occlusion_strength;
        m.emissive = p.glow;
        m.emissive_strength = GLTF_EMISSIVE_LUMINANCE * p.glow_strength;
        m.uv_scale = p.tiling;
        m.alpha_mode = match p.surface.as_str() {
            "cutout" => AlphaMode::Mask(p.cutoff),
            "blend" => AlphaMode::Blend,
            _ => AlphaMode::Opaque,
        };
        m.double_sided = p.two_sided;
    }
    world
        .get_resource_or_init::<Pending>()
        .0
        .insert(key.to_string(), (p.clone(), Instant::now()));
}

/// Write the material files whose edits have settled (Update).
pub fn flush(world: &mut World) {
    let Some(root) = world.get_resource::<AssetServer>().map(|s| s.root()) else {
        return;
    };
    let Some(mut pending) = world.get_resource_mut::<Pending>() else {
        return;
    };
    let ready: Vec<(String, Params)> = pending
        .0
        .iter()
        .filter(|(_, (_, at))| at.elapsed() >= SETTLE)
        .map(|(k, (p, _))| (k.clone(), p.clone()))
        .collect();
    for (key, p) in ready {
        pending.0.remove(&key);
        if let Err(e) = save(&root, &key, &p) {
            engine::core::log::warn!("material {key}: {e}");
        }
    }
}

/// Write every pending edit now (before a screenshot or quitting).
pub fn flush_all(world: &mut World) {
    if let Some(mut pending) = world.get_resource_mut::<Pending>() {
        for (_, at) in pending.0.values_mut() {
            *at -= SETTLE;
        }
    }
    flush(world);
}

/// The materials an entity and everything under it are drawn with, by key, in first-seen
/// order: material files (editable) and materials inside model files.
pub fn materials_under(world: &World, entity: Entity) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut stack = vec![entity];
    while let Some(e) = stack.pop() {
        if let Some(children) = world.get::<Children>(e) {
            stack.extend(children.iter().collect::<Vec<_>>().into_iter().rev());
        }
        if let Some(r) = world.get::<MeshRenderer>(e)
            && r.visible
            && let MaterialRef::Asset(k) = &r.material
            && !out.contains(k)
        {
            out.push(k.clone());
        }
    }
    out
}

/// How many objects in the scene are drawn with material `key` (`<file>#MaterialN`).
fn users(world: &mut World, key: &str) -> usize {
    let mut q = world.query::<&MeshRenderer>();
    q.iter(world)
        .filter(|r| matches!(&r.material, MaterialRef::Asset(k) if k == key))
        .count()
}

/// `"imported/materials/x/x.material.gltf#Material0"` → the file key, for material files.
fn material_file(key: &str) -> Option<&str> {
    let (file, label) = key.split_once('#')?;
    (crate::materials::is_material(file) && label == "Material0").then_some(file)
}

/// The Details panel's material sections for `entity`.
pub fn panel(ui: &mut egui::Ui, world: &mut World, editor: &mut Editor, entity: Entity) {
    let keys = materials_under(world, entity);
    if keys.is_empty() {
        return;
    }
    let Some(root) = world.get_resource::<AssetServer>().map(|s| s.root()) else {
        return;
    };
    for key in keys {
        match material_file(&key) {
            Some(file) => material_section(ui, world, editor, &root, file, &key),
            None => embedded_section(ui, world, editor, entity, &key),
        }
    }
}

fn material_section(
    ui: &mut egui::Ui,
    world: &mut World,
    editor: &mut Editor,
    root: &Path,
    file: &str,
    key: &str,
) {
    // The pending edit, else the file.
    let pending = world
        .get_resource::<Pending>()
        .and_then(|p| p.0.get(file))
        .map(|(p, _)| p.clone());
    let Ok((saved, maps)) = read(root, file) else {
        return;
    };
    let before = pending.unwrap_or(saved);
    let mut p = before.clone();
    let users = users(world, key);
    let name = crate::materials::display_name(file);
    let id = ui.make_persistent_id(("material", file));
    egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, true)
        .show_header(ui, |ui| {
            ui.label(egui::RichText::new(format!("🎨 {name}")).strong())
                .on_hover_text(file);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(crate::theme::dim(if users == 1 {
                    "1 object".to_string()
                } else {
                    format!("shared by {users} objects")
                }));
            });
        })
        .body(|ui| params_ui(ui, &mut p, maps));
    if p != before {
        let (Ok(b), Ok(a)) = (serde_json::to_value(&before), serde_json::to_value(&p)) else {
            return;
        };
        editor::apply(
            world,
            editor,
            Command::Material {
                key: file.to_string(),
                before: b,
                after: a,
            },
        );
    }
}

/// The sliders. "× map" where the value multiplies a map.
fn params_ui(ui: &mut egui::Ui, p: &mut Params, maps: Maps) {
    let times = |on: bool| if on { "× map" } else { "" };
    egui::Grid::new(ui.next_auto_id())
        .num_columns(2)
        .spacing([10.0, 5.0])
        .show(ui, |ui| {
            ui.label("Tint");
            ui.horizontal(|ui| {
                let mut rgba =
                    egui::Rgba::from_rgba_unmultiplied(p.tint[0], p.tint[1], p.tint[2], p.tint[3]);
                if egui::color_picker::color_edit_button_rgba(
                    ui,
                    &mut rgba,
                    egui::color_picker::Alpha::OnlyBlend,
                )
                .changed()
                {
                    p.tint = rgba.to_rgba_unmultiplied();
                }
                ui.label(crate::theme::dim(times(maps.color)));
            });
            ui.end_row();
            let slider = |ui: &mut egui::Ui, label: &str, v: &mut f32, max: f32, note: &str| {
                ui.label(label);
                ui.horizontal(|ui| {
                    ui.add(egui::Slider::new(v, 0.0..=max).max_decimals(2));
                    ui.label(crate::theme::dim(note));
                });
                ui.end_row();
            };
            slider(ui, "Roughness", &mut p.roughness, 1.0, times(maps.orm));
            slider(ui, "Metallic", &mut p.metallic, 1.0, times(maps.orm));
            if maps.normal {
                slider(ui, "Bumps", &mut p.normal_strength, 3.0, "normal map");
            }
            if maps.occlusion {
                slider(ui, "Occlusion", &mut p.occlusion_strength, 1.0, "");
            }
            ui.label("Tiling");
            ui.horizontal(|ui| {
                for (axis, v) in ["U", "V"].iter().zip(p.tiling.iter_mut()) {
                    ui.label(crate::theme::dim(*axis));
                    ui.add(
                        egui::DragValue::new(v)
                            .speed(0.02)
                            .range(0.01..=256.0)
                            .max_decimals(2),
                    );
                }
            });
            ui.end_row();
            ui.label("Glow");
            ui.horizontal(|ui| {
                let mut rgb = egui::Rgba::from_rgb(p.glow[0], p.glow[1], p.glow[2]);
                if egui::color_picker::color_edit_button_rgba(
                    ui,
                    &mut rgb,
                    egui::color_picker::Alpha::Opaque,
                )
                .changed()
                {
                    p.glow = [rgb.r(), rgb.g(), rgb.b()];
                }
                ui.add(
                    egui::DragValue::new(&mut p.glow_strength)
                        .speed(0.05)
                        .range(0.0..=100.0)
                        .prefix("× "),
                );
                ui.label(crate::theme::dim(times(maps.glow)));
            });
            ui.end_row();
            ui.label("Surface");
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt(ui.next_auto_id())
                    .selected_text(match p.surface.as_str() {
                        "cutout" => "Cutout",
                        "blend" => "See-through",
                        _ => "Opaque",
                    })
                    .show_ui(ui, |ui| {
                        for (v, label) in [
                            ("opaque", "Opaque"),
                            ("cutout", "Cutout"),
                            ("blend", "See-through"),
                        ] {
                            ui.selectable_value(&mut p.surface, v.to_string(), label);
                        }
                    });
                if p.surface == "cutout" {
                    ui.add(
                        egui::DragValue::new(&mut p.cutoff)
                            .speed(0.01)
                            .range(0.0..=1.0)
                            .prefix("below "),
                    );
                }
            });
            ui.end_row();
            ui.label("Two-sided");
            ui.checkbox(&mut p.two_sided, "");
            ui.end_row();
        });
}

/// A material inside a model file: its name and Make editable.
fn embedded_section(
    ui: &mut egui::Ui,
    world: &mut World,
    editor: &mut Editor,
    entity: Entity,
    key: &str,
) {
    let Some((file, index)) = key
        .rsplit_once("#Material")
        .and_then(|(f, i)| Some((f.to_string(), i.parse::<usize>().ok()?)))
    else {
        return;
    };
    if model_root(world, entity, &file).is_none() {
        return;
    }
    let Some(root) = world.get_resource::<AssetServer>().map(|s| s.root()) else {
        return;
    };
    let name = crate::remap::file_materials(&root.join(&file))
        .get(index)
        .map(|m| m.name.clone())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| format!("Material {index}"));
    let mut extract = false;
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(format!("🎨 {name}")).strong());
        ui.label(crate::theme::dim(format!(
            "inside {}",
            Path::new(&file)
                .file_name()
                .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
        )));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            extract = ui
                .button("✏ Make editable")
                .on_hover_text(
                    "Copy this material and its images out of the model into a material file \
                     of its own, and draw the model with that",
                )
                .clicked();
        });
    });
    ui.add_space(2.0);
    if extract {
        make_editable(world, editor, &file, index);
    }
}

/// The placed model (of file `file`) that `entity` belongs to.
fn model_root(world: &World, mut entity: Entity, file: &str) -> Option<Entity> {
    loop {
        if world.get::<crate::models::Model>(entity).is_some_and(|m| {
            engine::assets::normalize_path(&m.path) == engine::assets::normalize_path(file)
        }) {
            return Some(entity);
        }
        entity = world.get::<ChildOf>(entity)?.parent();
    }
}

/// Copy material `index` of model file `file` (and the images it uses) into a material file
/// and remap the model's copies to it. Returns the new material file's key.
pub fn make_editable(
    world: &mut World,
    editor: &mut Editor,
    file: &str,
    index: usize,
) -> Option<String> {
    let t = editor::now(world);
    let root = world.get_resource::<AssetServer>().map(|s| s.root())?;
    match extract(&root, file, index) {
        Ok((name, key)) => {
            let mut settings = crate::import::settings_for(&root, file).unwrap_or_default();
            settings.materials.insert(name.clone(), key.clone());
            editor.info(
                t,
                format!(
                    "{name} is a material file now: {}",
                    crate::materials::display_name(&key)
                ),
            );
            editor.models = None;
            crate::import::save_remaps(world, editor, file, &settings);
            Some(key)
        }
        Err(e) => {
            editor.error(t, format!("Could not make the material editable: {e}"));
            None
        }
    }
}

/// Write material `index` of glTF/GLB `file` as a material file. Returns (its name in the
/// model, the new key).
pub fn extract(root: &Path, file: &str, index: usize) -> Result<(String, String), String> {
    let path = root.join(file);
    let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let (json_bytes, bin): (Vec<u8>, Option<Vec<u8>>) = if bytes.starts_with(b"glTF") {
        let glb = gltf::Glb::from_slice(&bytes).map_err(|e| e.to_string())?;
        (glb.json.into_owned(), glb.bin.map(|b| b.into_owned()))
    } else {
        (bytes, None)
    };
    let doc: Value = serde_json::from_slice(&json_bytes).map_err(|e| e.to_string())?;
    let mut material = doc["materials"]
        .get(index)
        .cloned()
        .ok_or(format!("{file} has no material {index}"))?;
    let name = material["name"]
        .as_str()
        .filter(|n| !n.is_empty())
        .map_or_else(|| format!("Material {index}"), str::to_string);
    let model = Path::new(file)
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let slug = crate::import::slug(&format!("{model}_{name}"));
    let folder = format!(
        "{}/{}/{slug}",
        crate::import::IMPORTED_DIR,
        crate::materials::MATERIALS_DIR
    );
    let dir = root.join(&folder);
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let source_dir = path.parent().unwrap_or(Path::new("."));
    // Copy each texture's image out, renumbering textures and images.
    let mut images: Vec<Value> = Vec::new();
    let mut textures: Vec<Value> = Vec::new();
    let mut moved: BTreeMap<u64, usize> = BTreeMap::new();
    for info in texture_infos(&mut material) {
        let Some(old) = info["index"].as_u64() else {
            continue;
        };
        let new = match moved.get(&old) {
            Some(&n) => n,
            None => {
                let image = &doc["images"][doc["textures"][old as usize]["source"]
                    .as_u64()
                    .unwrap_or(u64::MAX) as usize];
                let (data, ext) = image_bytes(&doc, image, bin.as_deref(), source_dir)?;
                let out = format!("{slug}_{}.{ext}", images.len());
                std::fs::write(dir.join(&out), data).map_err(|e| e.to_string())?;
                images.push(json!({ "uri": out }));
                textures.push(json!({ "source": images.len() - 1 }));
                moved.insert(old, textures.len() - 1);
                textures.len() - 1
            }
        };
        info["index"] = json!(new);
        if let Some(obj) = info.as_object_mut() {
            obj.remove("texCoord");
        }
    }
    let mut out = json!({
        "asset": { "version": "2.0", "generator": "Engine editor (material made editable)" },
        "materials": [material],
    });
    if !images.is_empty() {
        out["images"] = json!(images);
        out["textures"] = json!(textures);
    }
    note_extensions(&mut out);
    let name_file = format!("{slug}{}", crate::materials::MATERIAL_SUFFIX);
    let text = serde_json::to_string_pretty(&out).map_err(|e| e.to_string())?;
    std::fs::write(dir.join(&name_file), text).map_err(|e| e.to_string())?;
    Ok((name, format!("{folder}/{name_file}")))
}

/// An image's encoded bytes and file extension: from the GLB's binary chunk, a file beside
/// the model, or a data URI.
fn image_bytes(
    doc: &Value,
    image: &Value,
    bin: Option<&[u8]>,
    dir: &Path,
) -> Result<(Vec<u8>, &'static str), String> {
    let data = if let Some(view) = image["bufferView"].as_u64() {
        let view = &doc["bufferViews"][view as usize];
        let buffer = view["buffer"].as_u64().unwrap_or(0) as usize;
        let start = view["byteOffset"].as_u64().unwrap_or(0) as usize;
        let len = view["byteLength"].as_u64().unwrap_or(0) as usize;
        let whole = match doc["buffers"][buffer]["uri"].as_str() {
            Some(uri) => std::fs::read(dir.join(uri)).map_err(|e| format!("{uri}: {e}"))?,
            None => bin.ok_or("no binary chunk")?.to_vec(),
        };
        whole
            .get(start..start + len)
            .ok_or("image outside its buffer")?
            .to_vec()
    } else if let Some(uri) = image["uri"].as_str() {
        if uri.starts_with("data:") {
            return Err("embedded data URI images aren't copied yet".into());
        }
        let decoded = uri.replace("%20", " ");
        std::fs::read(dir.join(&decoded)).map_err(|e| format!("{uri}: {e}"))?
    } else {
        return Err("an image with no data".into());
    };
    let ext = match image::guess_format(&data) {
        Ok(image::ImageFormat::Jpeg) => "jpg",
        _ => "png",
    };
    Ok((data, ext))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn params_round_trip_through_gltf_json() {
        let mut m = json!({
            "name": "Stone",
            "pbrMetallicRoughness": {
                "baseColorTexture": { "index": 0 },
                "metallicRoughnessTexture": { "index": 1 },
                "metallicFactor": 1.0, "roughnessFactor": 1.0
            },
            "normalTexture": { "index": 2 }
        });
        let (p, maps) = from_json(&m);
        assert_eq!(
            p,
            Params {
                ..Params::default()
            }
        );
        assert!(maps.color && maps.orm && maps.normal && !maps.occlusion);
        let edited = Params {
            tint: [0.8, 0.7, 0.6, 1.0],
            roughness: 0.4,
            normal_strength: 1.5,
            tiling: [3.0, 2.0],
            surface: "cutout".into(),
            cutoff: 0.3,
            two_sided: true,
            glow: [1.0, 0.5, 0.0],
            glow_strength: 4.0,
            ..Params::default()
        };
        to_json(&mut m, &edited);
        // Textures stay, every map gets the tiling.
        assert_eq!(m["pbrMetallicRoughness"]["baseColorTexture"]["index"], 0);
        assert_eq!(
            m["normalTexture"]["extensions"]["KHR_texture_transform"]["scale"],
            json!([3.0, 2.0])
        );
        assert_eq!(from_json(&m).0, edited);
        // Back to plain: the extensions go again.
        to_json(&mut m, &Params::default());
        assert!(m.get("extensions").is_none());
        assert!(
            m["normalTexture"]
                .get("extensions")
                .is_none_or(|e| e.as_object().is_some_and(|o| o.is_empty()))
        );
        assert_eq!(from_json(&m).0, Params::default());
    }

    #[test]
    fn a_glb_material_and_its_image_come_out() {
        // A one-material GLB holding a 1x1 PNG.
        let mut png = Vec::new();
        image::RgbaImage::from_pixel(1, 1, image::Rgba([200, 30, 30, 255]))
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let doc = json!({
            "asset": { "version": "2.0" },
            "buffers": [{ "byteLength": png.len() }],
            "bufferViews": [{ "buffer": 0, "byteOffset": 0, "byteLength": png.len() }],
            "images": [{ "bufferView": 0, "mimeType": "image/png" }],
            "textures": [{ "source": 0 }],
            "materials": [
                { "name": "Plain" },
                { "name": "Painted", "pbrMetallicRoughness": { "baseColorTexture": { "index": 0 } } }
            ]
        });
        let json_bytes = serde_json::to_vec(&doc).unwrap();
        let glb = gltf::Glb {
            header: gltf::binary::Header {
                magic: *b"glTF",
                version: 2,
                length: 0,
            },
            json: std::borrow::Cow::Owned(json_bytes),
            bin: Some(std::borrow::Cow::Owned(png.clone())),
        };
        let root = std::env::temp_dir().join(format!("editor-extract-{}", std::process::id()));
        std::fs::create_dir_all(root.join("imported/box")).unwrap();
        std::fs::write(root.join("imported/box/Box.glb"), glb.to_vec().unwrap()).unwrap();
        let (name, key) = extract(&root, "imported/box/Box.glb", 1).unwrap();
        assert_eq!(name, "Painted");
        assert_eq!(
            key,
            "imported/materials/box_painted/box_painted.material.gltf"
        );
        let (p, maps) = read(&root, &key).unwrap();
        assert!(maps.color);
        assert_eq!(p, Params::default());
        let copied = std::fs::read(root.join("imported/materials/box_painted/box_painted_0.png"));
        assert_eq!(copied.unwrap(), png);
        std::fs::remove_dir_all(&root).ok();
    }
}
