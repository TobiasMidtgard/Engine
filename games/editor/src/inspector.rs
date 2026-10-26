//! The Details panel: every registered component of the selected entity (and every registered
//! resource, in World Settings), edited as typed widgets built from its JSON.
//!
//! Nothing here knows module types: numbers become drag fields, `[x, y, z]` arrays become vector
//! rows, fields named like colors get color pickers, enums get a dropdown of their variants (the
//! variant names come from asking serde, see [`enum_variants`]), and nested structs fold out. So
//! a component a module registers tomorrow is editable without editor changes.

use engine::core::registry::{ComponentEntry, TypeRegistry};
use engine::prelude::*;
use serde_json::{Map, Number, Value, json};

use crate::editor::{self, Editor};
use crate::theme;
use crate::undo::Command;

/// What an edit applies to.
#[derive(Clone)]
pub enum Target {
    Component(Entity, ComponentEntry),
    Resource(String),
}

/// Draw the whole inspector for the primary selection.
pub fn entity_details(ui: &mut egui::Ui, world: &mut World, editor: &mut Editor) {
    let Some(entity) = editor.primary() else {
        ui.add_space(24.0);
        ui.vertical_centered(|ui| {
            ui.label(theme::dim("Select something in the scene or the outliner"));
        });
        return;
    };
    if world.get_entity(entity).is_err() {
        editor.selection.retain(|&e| e != entity);
        return;
    }

    // Header: name, kind, id.
    let in_scene = world.get::<crate::scene_ops::InScene>(entity).is_some();
    let mut name = world
        .get::<Name>(entity)
        .map(|n| n.as_str().to_string())
        .unwrap_or_default();
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(theme::entity_icon(world, entity)).size(18.0));
        let edit = ui.add(
            egui::TextEdit::singleline(&mut name)
                .hint_text("Name")
                .desired_width(ui.available_width() - 8.0),
        );
        if edit.changed() {
            let before = world.get::<Name>(entity).map(|n| n.as_str().to_string());
            editor::apply(
                world,
                editor,
                Command::Rename {
                    entity,
                    before,
                    after: (!name.is_empty()).then(|| name.clone()),
                },
            );
        }
    });
    ui.horizontal(|ui| {
        let kind = if in_scene {
            "Scene entity"
        } else if world.get::<crate::demos::DemoPart>(entity).is_some() {
            "Spawned by a module demo (not saved)"
        } else {
            "Runtime entity (not saved)"
        };
        ui.label(theme::dim(kind));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(theme::dim(format!("{entity}")));
        });
    });
    if editor.selection.len() > 1 {
        ui.label(theme::dim(format!(
            "{} selected; editing the last one",
            editor.selection.len()
        )));
    }
    ui.add_space(4.0);

    quick_actions(ui, world, editor, entity);

    let registry = world.resource::<TypeRegistry>().clone();
    let mut entries: Vec<&ComponentEntry> = registry
        .components()
        .filter(|c| c.name != "Name" && (c.has)(world, entity))
        .collect();
    // What the entity is first (transform, light, demo, body), the big render data last.
    let rank = |name: &str| match name {
        "Transform" => 0,
        "ModuleDemo" | "PointLight" | "DirectionalLight" | "Camera" => 1,
        "RigidBody" | "Collider" => 2,
        "MeshRenderer" => 4,
        _ => 3,
    };
    entries.sort_by_key(|c| (rank(&c.name), c.name.clone()));
    for entry in entries {
        let Some(value) = (entry.get)(world, entity) else {
            continue;
        };
        let id = ui.make_persistent_id(("component", entry.name.as_str()));
        let mut remove = false;
        egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, true)
            .show_header(ui, |ui| {
                ui.label(egui::RichText::new(&entry.name).strong())
                    .on_hover_text(&entry.doc);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if entry.name != "Transform"
                        && ui
                            .small_button("✖")
                            .on_hover_text(format!("Remove {}", entry.name))
                            .clicked()
                    {
                        remove = true;
                    }
                });
            })
            .body(|ui| {
                let target = Target::Component(entity, entry.clone());
                if entry.name == "Transform" {
                    transform_editor(ui, world, editor, entity, &value);
                } else {
                    edit_root(ui, world, editor, &target, &value);
                }
            });
        if remove {
            editor::apply(
                world,
                editor,
                Command::Component {
                    entity,
                    name: entry.name.clone(),
                    before: Some(value),
                    after: None,
                },
            );
        }
    }
    ui.add_space(8.0);
    add_component_button(ui, world, editor, entity, &registry);
}

/// One-click setups that span modules (they only add registered components).
fn quick_actions(ui: &mut egui::Ui, world: &mut World, editor: &mut Editor, entity: Entity) {
    let has = |world: &World, name: &str| {
        world
            .resource::<TypeRegistry>()
            .component(name)
            .is_some_and(|c| (c.has)(world, entity))
    };
    let has_mesh = world.get::<MeshRenderer>(entity).is_some();
    let can_body = has_mesh
        && !has(world, "RigidBody")
        && world
            .resource::<TypeRegistry>()
            .component("RigidBody")
            .is_some();
    let is_demo = has(world, "ModuleDemo");
    let prefab = world
        .get::<crate::prefab::PrefabInstance>(entity)
        .map(|p| p.path.clone());
    if !can_body && !is_demo && prefab.is_none() {
        return;
    }
    ui.horizontal_wrapped(|ui| {
        if can_body {
            if ui
                .button("⚙ Make physical")
                .on_hover_text("Add a dynamic RigidBody and a Collider fitted to the mesh")
                .clicked()
            {
                add_physics(world, editor, entity, true);
            }
            if ui
                .button("▣ Make solid")
                .on_hover_text("Add a fixed RigidBody and a Collider fitted to the mesh")
                .clicked()
            {
                add_physics(world, editor, entity, false);
            }
        }
        if let Some(path) = &prefab {
            if ui
                .button("📦 Apply to prefab")
                .on_hover_text(format!(
                    "Save this to {path} and update every other instance"
                ))
                .clicked()
            {
                crate::prefab::apply_changes(world, editor, entity);
            }
            if ui
                .button("↻ Revert")
                .on_hover_text(format!("Rebuild this from {path}"))
                .clicked()
            {
                crate::prefab::revert(world, editor, entity);
            }
        }
        if is_demo && ui.button("↻ Respawn demo").clicked() {
            world
                .entity_mut(entity)
                .remove::<crate::demos::DemoInstance>();
        }
    });
    ui.add_space(4.0);
}

/// Add a body and a box collider fitted to the entity's mesh bounds.
fn add_physics(world: &mut World, editor: &mut Editor, entity: Entity, dynamic: bool) {
    let mesh = world
        .get::<engine::render::ResolvedMesh>(entity)
        .and_then(|r| world.resource::<Assets<engine::render::Mesh>>().get(r.mesh))
        .map(|m| m.aabb());
    let half = mesh
        .map(|a| (a.max - a.min) * 0.5)
        .unwrap_or(Vec3::splat(0.5));
    let body = if dynamic {
        engine::world::physics::RigidBody::dynamic()
    } else {
        engine::world::physics::RigidBody::fixed()
    };
    let collider = engine::world::physics::Collider::cuboid(half);
    let commands = vec![
        Command::Component {
            entity,
            name: "RigidBody".into(),
            before: None,
            after: serde_json::to_value(body).ok(),
        },
        Command::Component {
            entity,
            name: "Collider".into(),
            before: None,
            after: serde_json::to_value(collider).ok(),
        },
    ];
    editor::apply(
        world,
        editor,
        Command::Group("Add physics".into(), commands),
    );
}

fn add_component_button(
    ui: &mut egui::Ui,
    world: &mut World,
    editor: &mut Editor,
    entity: Entity,
    registry: &TypeRegistry,
) {
    let button = ui.add_sized(
        [ui.available_width(), 26.0],
        egui::Button::new(egui::RichText::new("➕ Add Component").strong()),
    );
    egui::Popup::menu(&button)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            ui.set_min_width(300.0);
            ui.add(
                egui::TextEdit::singleline(&mut editor.add_filter)
                    .hint_text("Search components…")
                    .desired_width(f32::INFINITY),
            )
            .request_focus();
            let filter = editor.add_filter.to_lowercase();
            egui::ScrollArea::vertical()
                .max_height(360.0)
                .show(ui, |ui| {
                    for entry in registry.components() {
                        if entry.name == "Name"
                            || (entry.has)(world, entity)
                            || (!filter.is_empty()
                                && !entry.name.to_lowercase().contains(&filter)
                                && !entry.doc.to_lowercase().contains(&filter))
                        {
                            continue;
                        }
                        let r = ui
                            .add(egui::Button::new(&entry.name).frame(false))
                            .on_hover_text(&entry.doc);
                        if r.clicked() {
                            editor::apply(
                                world,
                                editor,
                                Command::Component {
                                    entity,
                                    name: entry.name.clone(),
                                    before: None,
                                    after: Some(json!({})),
                                },
                            );
                            editor.add_filter.clear();
                            ui.close();
                        }
                    }
                });
        });
}

/// Every registered resource, editable (World Settings tab).
pub fn resources(ui: &mut egui::Ui, world: &mut World, editor: &mut Editor) {
    let registry = world.resource::<TypeRegistry>().clone();
    ui.add(
        egui::TextEdit::singleline(&mut editor.outliner_filter)
            .hint_text("Filter settings…")
            .desired_width(f32::INFINITY),
    );
    let filter = editor.outliner_filter.to_lowercase();
    for entry in registry.resources() {
        if !filter.is_empty() && !entry.name.to_lowercase().contains(&filter) {
            continue;
        }
        let Some(value) = (entry.get)(world) else {
            continue;
        };
        egui::CollapsingHeader::new(egui::RichText::new(&entry.name).strong())
            .id_salt(("resource", entry.name.as_str()))
            .default_open(false)
            .show(ui, |ui| {
                ui.label(theme::dim(&entry.doc));
                edit_root(
                    ui,
                    world,
                    editor,
                    &Target::Resource(entry.name.clone()),
                    &value,
                );
            })
            .header_response
            .on_hover_text(&entry.doc);
    }
}

/// Edit a whole component or resource value; applies a command when anything changed.
pub fn edit_root(
    ui: &mut egui::Ui,
    world: &mut World,
    editor: &mut Editor,
    target: &Target,
    value: &Value,
) {
    let mut edited = value.clone();
    let mut ctx = EditCtx {
        world,
        editor,
        target,
        root: value,
    };
    let changed = match &mut edited {
        Value::Object(map) => edit_fields(ui, &mut ctx, "", map),
        other => edit_value(ui, &mut ctx, "", "value", other),
    };
    if changed {
        commit(world, editor, target, value.clone(), edited);
    }
}

fn commit(world: &mut World, editor: &mut Editor, target: &Target, before: Value, after: Value) {
    let command = match target {
        Target::Component(entity, entry) => Command::Component {
            entity: *entity,
            name: entry.name.clone(),
            before: Some(before),
            after: Some(after),
        },
        Target::Resource(name) => Command::Resource {
            name: name.clone(),
            before,
            after,
        },
    };
    editor::apply(world, editor, command);
}

struct EditCtx<'a> {
    world: &'a mut World,
    editor: &'a mut Editor,
    target: &'a Target,
    root: &'a Value,
}

fn edit_fields(
    ui: &mut egui::Ui,
    ctx: &mut EditCtx,
    path: &str,
    map: &mut Map<String, Value>,
) -> bool {
    let mut changed = false;
    let keys: Vec<String> = map.keys().cloned().collect();
    for key in keys {
        let child_path = format!("{path}/{key}");
        let Some(value) = map.get_mut(&key) else {
            continue;
        };
        if is_struct(value) {
            // Nested structs fold out under their own header.
            let Value::Object(inner) = value else {
                continue;
            };
            let open = inner.len() <= 6;
            let r = egui::CollapsingHeader::new(pretty(&key))
                .id_salt(("struct", child_path.as_str()))
                .default_open(open)
                .show(ui, |ui| edit_fields(ui, ctx, &child_path, inner));
            changed |= r.body_returned == Some(true);
            continue;
        }
        ui.horizontal(|ui| {
            let label_w = (ui.available_width() * 0.36).clamp(80.0, 170.0);
            ui.allocate_ui_with_layout(
                egui::vec2(label_w, 20.0),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    ui.set_min_width(label_w);
                    ui.add(egui::Label::new(pretty(&key)).truncate())
                        .on_hover_text(&key);
                },
            );
            changed |= edit_value(ui, ctx, &child_path, &key, value);
        });
    }
    changed
}

fn is_struct(v: &Value) -> bool {
    // An object with one capitalized key is an enum variant, not a struct.
    matches!(v, Value::Object(m) if !m.is_empty() && !is_enum_object(m))
}

fn is_enum_object(m: &Map<String, Value>) -> bool {
    m.len() == 1
        && m.keys().next().is_some_and(|k| {
            k.chars().next().is_some_and(|c| c.is_uppercase()) || is_snake_variant(k)
        })
}

/// `MeshRef`/`MaterialRef` style snake_case variant names (`shape`, `inline`, `asset`).
fn is_snake_variant(k: &str) -> bool {
    matches!(k, "shape" | "inline" | "asset" | "handle")
}

/// "perceptual_roughness" → "Perceptual roughness".
pub fn pretty(key: &str) -> String {
    let s = key.replace('_', " ");
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => s,
    }
}

fn edit_value(
    ui: &mut egui::Ui,
    ctx: &mut EditCtx,
    path: &str,
    key: &str,
    value: &mut Value,
) -> bool {
    match value {
        Value::Bool(b) => ui.checkbox(b, "").changed(),
        Value::Number(n) => edit_number(ui, key, n),
        Value::String(s) => edit_string(ui, ctx, path, s),
        Value::Null => {
            ui.label(theme::dim("None"));
            false
        }
        Value::Array(items) => edit_array(ui, ctx, path, key, items),
        Value::Object(map) if is_enum_object(map) => edit_enum_object(ui, ctx, path, map),
        Value::Object(map) => {
            let mut changed = false;
            ui.vertical(|ui| changed = edit_fields(ui, ctx, path, map));
            changed
        }
    }
}

fn edit_number(ui: &mut egui::Ui, key: &str, n: &mut Number) -> bool {
    edit_number_sized(ui, key, n, None)
}

/// Width for each of `n` fields sharing the rest of the row, each after a one-letter label.
fn field_width(ui: &egui::Ui, n: usize) -> f32 {
    let spacing = ui.spacing().item_spacing.x;
    ((ui.available_width() - n as f32 * (10.0 + 2.0 * spacing)) / n as f32).clamp(28.0, 90.0)
}

fn add_sized_opt(ui: &mut egui::Ui, width: Option<f32>, w: impl egui::Widget) -> egui::Response {
    match width {
        Some(width) => ui.add_sized([width, 20.0], w),
        None => ui.add(w),
    }
}

fn edit_number_sized(ui: &mut egui::Ui, key: &str, n: &mut Number, width: Option<f32>) -> bool {
    if let Some(i) = n.as_i64().filter(|_| !n.is_f64()) {
        let mut v = i;
        let r = add_sized_opt(ui, width, egui::DragValue::new(&mut v).speed(0.1));
        if r.changed() {
            *n = Number::from(v);
            return true;
        }
        return false;
    }
    let mut v = n.as_f64().unwrap_or(0.0);
    let speed = (v.abs() * 0.01).clamp(0.001, 1000.0);
    let mut drag = egui::DragValue::new(&mut v).speed(speed).max_decimals(4);
    if key.ends_with("_deg") {
        drag = drag.suffix("°");
    } else if key.ends_with("_m") || key == "range" || key == "radius" || key == "height" {
        drag = drag.suffix(" m");
    } else if key.ends_with("_kg") {
        drag = drag.suffix(" kg");
    } else if key.ends_with("_c") {
        drag = drag.suffix(" °C");
    }
    let r = add_sized_opt(ui, width, drag);
    if r.changed()
        && let Some(new) = Number::from_f64(v)
    {
        *n = new;
        return true;
    }
    false
}

fn edit_string(ui: &mut egui::Ui, ctx: &mut EditCtx, path: &str, s: &mut String) -> bool {
    // A string that is one of a unit enum's variants (or a lowercase key a type checks, like a
    // module demo's) gets a dropdown.
    let word = !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    let variants = if word || s.chars().next().is_some_and(|c| c.is_uppercase()) {
        enum_variants(ctx, path)
    } else {
        None
    };
    if let Some(variants) = variants {
        let mut changed = false;
        egui::ComboBox::from_id_salt(("enum", path))
            .selected_text(s.as_str())
            .show_ui(ui, |ui| {
                for v in &variants {
                    if ui.selectable_label(s == v, v).clicked() && s != v {
                        *s = v.clone();
                        changed = true;
                    }
                }
            });
        return changed;
    }
    ui.add(egui::TextEdit::singleline(s).desired_width(180.0))
        .changed()
}

fn edit_enum_object(
    ui: &mut egui::Ui,
    ctx: &mut EditCtx,
    path: &str,
    map: &mut Map<String, Value>,
) -> bool {
    let Some((variant, inner)) = map.iter_mut().next() else {
        return false;
    };
    let variant = variant.clone();
    let mut changed = false;
    let mut switch_to = None;
    ui.vertical(|ui| {
        match enum_variants(ctx, path) {
            Some(variants) => {
                egui::ComboBox::from_id_salt(("enum", path))
                    .selected_text(&variant)
                    .show_ui(ui, |ui| {
                        for v in &variants {
                            if ui.selectable_label(&variant == v, v).clicked() && &variant != v {
                                switch_to = Some(v.clone());
                            }
                        }
                    });
            }
            None => {
                ui.label(egui::RichText::new(&variant).color(theme::ACCENT));
            }
        }
        let inner_path = format!("{path}/{variant}");
        changed = match inner {
            Value::Object(fields) if fields.len() > 8 => {
                let r = egui::CollapsingHeader::new(format!("{} fields", fields.len()))
                    .id_salt(("variant", inner_path.as_str()))
                    .default_open(false)
                    .show(ui, |ui| edit_fields(ui, ctx, &inner_path, fields));
                r.body_returned == Some(true)
            }
            Value::Object(fields) => edit_fields(ui, ctx, &inner_path, fields),
            other => edit_value(ui, ctx, &inner_path, &variant, other),
        };
    });
    if let Some(v) = switch_to {
        map.clear();
        map.insert(v, json!({}));
        return true;
    }
    changed
}

fn edit_array(
    ui: &mut egui::Ui,
    ctx: &mut EditCtx,
    path: &str,
    key: &str,
    items: &mut Vec<Value>,
) -> bool {
    let all_numbers = !items.is_empty() && items.iter().all(|v| v.is_number());
    if all_numbers && (2..=4).contains(&items.len()) {
        if is_color_key(key) && items.len() >= 3 {
            return edit_color(ui, items);
        }
        let labels = ["X", "Y", "Z", "W"];
        let mut changed = false;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 3.0;
            let width = field_width(ui, items.len());
            for (i, item) in items.iter_mut().enumerate() {
                if let Value::Number(n) = item {
                    ui.label(
                        egui::RichText::new(labels[i])
                            .color(theme::AXIS_COLORS[i])
                            .small(),
                    );
                    changed |= edit_number_sized(ui, key, n, Some(width));
                }
            }
        });
        return changed;
    }
    let mut changed = false;
    ui.vertical(|ui| {
        let header = format!(
            "{} item{}",
            items.len(),
            if items.len() == 1 { "" } else { "s" }
        );
        egui::CollapsingHeader::new(header)
            .id_salt(("array", path))
            .default_open(items.len() <= 3)
            .show(ui, |ui| {
                let mut remove = None;
                // Long lists (a landscape's brush strokes) show their first entries only.
                let shown = items.len().min(MAX_ARRAY_ROWS);
                for (i, item) in items.iter_mut().enumerate().take(shown) {
                    ui.horizontal(|ui| {
                        ui.label(theme::dim(format!("{i}")));
                        if ui.small_button("−").on_hover_text("Remove").clicked() {
                            remove = Some(i);
                        }
                        changed |= edit_value(ui, ctx, &format!("{path}/{i}"), key, item);
                    });
                }
                if items.len() > shown {
                    ui.label(theme::dim(format!("…and {} more", items.len() - shown)));
                }
                if let Some(i) = remove {
                    items.remove(i);
                    changed = true;
                }
                if let Some(last) = items.last().cloned()
                    && ui.small_button("➕ Add").clicked()
                {
                    items.push(last);
                    changed = true;
                }
            });
    });
    changed
}

/// Rows an array shows before "…and N more".
const MAX_ARRAY_ROWS: usize = 40;

fn is_color_key(key: &str) -> bool {
    let k = key.to_lowercase();
    k.contains("color") || k.contains("colour") || k == "albedo" || k == "emissive" || k == "tint"
}

fn edit_color(ui: &mut egui::Ui, items: &mut [Value]) -> bool {
    let get = |v: &Value| v.as_f64().unwrap_or(0.0) as f32;
    let alpha = items.len() == 4;
    let mut rgba = egui::Rgba::from_rgba_premultiplied(
        get(&items[0]),
        get(&items[1]),
        get(&items[2]),
        if alpha { get(&items[3]) } else { 1.0 },
    );
    // Values above 1 (HDR tints) are kept as numbers; the picker shows them clamped.
    let mut changed = false;
    ui.horizontal(|ui| {
        let r = egui::widgets::color_picker::color_edit_button_rgba(
            ui,
            &mut rgba,
            if alpha {
                egui::color_picker::Alpha::OnlyBlend
            } else {
                egui::color_picker::Alpha::Opaque
            },
        );
        if r.changed() {
            let new = [rgba.r(), rgba.g(), rgba.b(), rgba.a()];
            for (i, item) in items.iter_mut().enumerate() {
                if let Some(n) = Number::from_f64(new[i] as f64) {
                    *item = Value::Number(n);
                }
            }
            changed = true;
        }
        ui.spacing_mut().item_spacing.x = 3.0;
        let width = field_width(ui, items.len()) + 10.0;
        for item in items.iter_mut() {
            if let Value::Number(n) = item {
                changed |= edit_number_sized(ui, "", n, Some(width));
            }
        }
    });
    changed
}

/// Variant names of the enum at `path` inside the edited value, or `None` when it isn't an
/// enum. Found by writing a nonsense variant there and reading serde's "unknown variant `x`,
/// expected one of `A`, `B`" error, which names every variant. Cached per type and path.
fn enum_variants(ctx: &mut EditCtx, path: &str) -> Option<Vec<String>> {
    let type_name = match ctx.target {
        Target::Component(_, entry) => entry.name.clone(),
        Target::Resource(name) => format!("resource {name}"),
    };
    // Array indices don't change the type: cache "Foo/items/3/kind" as "Foo/items/#/kind".
    let generic: String = path
        .split('/')
        .map(|p| if p.parse::<usize>().is_ok() { "#" } else { p })
        .collect::<Vec<_>>()
        .join("/");
    let cache_key = format!("{type_name}:{generic}");
    if let Some(cached) = ctx.editor.enum_cache.get(&cache_key) {
        return cached.clone();
    }
    let mut probe = ctx.root.clone();
    let found = set_at(&mut probe, path, Value::String("\u{1}probe".into()))
        .then(|| try_deserialize(ctx.world, ctx.target, probe))
        .flatten()
        .and_then(|err| parse_variants(&err));
    ctx.editor.enum_cache.insert(cache_key, found.clone());
    found
}

/// Replace the value at a `/a/b/0` path; false when the path doesn't exist.
fn set_at(root: &mut Value, path: &str, new: Value) -> bool {
    let mut cur = root;
    for part in path.split('/').filter(|p| !p.is_empty()) {
        cur = match cur {
            Value::Object(m) => match m.get_mut(part) {
                Some(v) => v,
                None => return false,
            },
            Value::Array(a) => match part.parse::<usize>().ok().and_then(|i| a.get_mut(i)) {
                Some(v) => v,
                None => return false,
            },
            _ => return false,
        };
    }
    // An enum object `{"Variant": {...}}`: probe the whole object, not its inner value.
    *cur = new;
    true
}

/// Deserialize `value` as the target's type without keeping it; returns the error text.
fn try_deserialize(world: &mut World, target: &Target, value: Value) -> Option<String> {
    match target {
        Target::Component(_, entry) => {
            // Never touches a real entity: deserializing fails before anything is inserted.
            let scratch = world.spawn_empty().id();
            let result = (entry.insert)(world, scratch, value);
            world.despawn(scratch);
            result.err()
        }
        Target::Resource(name) => {
            let entry = world.resource::<TypeRegistry>().resource(name)?.clone();
            let before = (entry.get)(world);
            let result = (entry.patch)(world, value);
            if result.is_ok()
                && let Some(b) = before
            {
                // It parsed (no enum there): put the old value back.
                let _ = (entry.patch)(world, b);
            }
            result.err()
        }
    }
}

/// Pull `A`, `B`, `C` out of "unknown variant `x`, expected one of `A`, `B`, `C`".
pub fn parse_variants(err: &str) -> Option<Vec<String>> {
    let rest = err.split("unknown variant").nth(1)?;
    let expected = rest.split("expected").nth(1)?;
    let names: Vec<String> = expected
        .split('`')
        .skip(1)
        .step_by(2)
        .map(str::to_string)
        .collect();
    (!names.is_empty()).then_some(names)
}

/// Transform with Euler angles in degrees and a uniform-scale lock.
fn transform_editor(
    ui: &mut egui::Ui,
    world: &mut World,
    editor: &mut Editor,
    entity: Entity,
    value: &Value,
) {
    let Ok(mut t) = serde_json::from_value::<Transform>(value.clone()) else {
        return;
    };
    let mut changed = false;
    egui::Grid::new(("transform", entity))
        .num_columns(2)
        .spacing([10.0, 4.0])
        .min_col_width(70.0)
        .show(ui, |ui| {
            ui.label("Position");
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 3.0;
                let w = field_width(ui, 3);
                for (i, v) in [
                    &mut t.translation.x,
                    &mut t.translation.y,
                    &mut t.translation.z,
                ]
                .into_iter()
                .enumerate()
                {
                    ui.label(
                        egui::RichText::new(["X", "Y", "Z"][i])
                            .color(theme::AXIS_COLORS[i])
                            .small(),
                    );
                    changed |= ui
                        .add_sized(
                            [w, 20.0],
                            egui::DragValue::new(v).speed(0.01).max_decimals(3),
                        )
                        .changed();
                }
            });
            ui.end_row();

            ui.label("Rotation");
            let (y, x, z) = t.rotation.to_euler(glam::EulerRot::YXZ);
            let mut deg = [x.to_degrees(), y.to_degrees(), z.to_degrees()];
            let mut rot_changed = false;
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 3.0;
                let w = field_width(ui, 3);
                for (i, v) in deg.iter_mut().enumerate() {
                    ui.label(
                        egui::RichText::new(["X", "Y", "Z"][i])
                            .color(theme::AXIS_COLORS[i])
                            .small(),
                    );
                    rot_changed |= ui
                        .add_sized(
                            [w, 20.0],
                            egui::DragValue::new(v)
                                .speed(0.5)
                                .max_decimals(2)
                                .suffix("°"),
                        )
                        .changed();
                }
            });
            if rot_changed {
                t.rotation = Quat::from_euler(
                    glam::EulerRot::YXZ,
                    deg[1].to_radians(),
                    deg[0].to_radians(),
                    deg[2].to_radians(),
                );
                changed = true;
            }
            ui.end_row();

            ui.label("Scale");
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 3.0;
                let w = field_width(ui, 3);
                for (i, v) in [&mut t.scale.x, &mut t.scale.y, &mut t.scale.z]
                    .into_iter()
                    .enumerate()
                {
                    ui.label(
                        egui::RichText::new(["X", "Y", "Z"][i])
                            .color(theme::AXIS_COLORS[i])
                            .small(),
                    );
                    changed |= ui
                        .add_sized(
                            [w, 20.0],
                            egui::DragValue::new(v).speed(0.01).max_decimals(3),
                        )
                        .changed();
                }
            });
            ui.end_row();
        });
    if changed && let Ok(after) = serde_json::to_value(t) {
        editor::apply(
            world,
            editor,
            Command::Component {
                entity,
                name: "Transform".into(),
                before: Some(value.clone()),
                after: Some(after),
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_variant_names_from_serde_errors() {
        let err = "invalid value for Camera: unknown variant `\u{1}probe`, expected `Perspective` or `Orthographic`";
        assert_eq!(
            parse_variants(err).unwrap(),
            vec!["Perspective".to_string(), "Orthographic".to_string()]
        );
        let err = "unknown variant `x`, expected one of `Dynamic`, `Kinematic`, `Static` at line 1";
        assert_eq!(parse_variants(err).unwrap().len(), 3);
        assert!(parse_variants("missing field `x`").is_none());
    }

    #[test]
    fn real_enum_errors_name_their_variants() {
        // What serde_json says for the engine's own enums.
        let err = serde_json::from_value::<engine::world::physics::BodyKind>(json!("nope"))
            .unwrap_err()
            .to_string();
        assert_eq!(
            parse_variants(&err).unwrap(),
            vec!["Dynamic", "Kinematic", "Static"]
        );
    }

    #[test]
    fn set_at_follows_paths() {
        let mut v = json!({"a": {"b": [1, {"c": "X"}]}});
        assert!(set_at(&mut v, "/a/b/1/c", json!("Y")));
        assert_eq!(v["a"]["b"][1]["c"], "Y");
        assert!(!set_at(&mut v, "/a/zz", json!(1)));
    }

    #[test]
    fn pretty_names() {
        assert_eq!(pretty("perceptual_roughness"), "Perceptual roughness");
        assert_eq!(pretty("x"), "X");
    }
}
