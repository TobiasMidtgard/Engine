//! How prefabs show in the editor: the Details panel's prefab bar and Overrides list (Unity's
//! Overrides dropdown: each change with the file's value beside it, applied to the copy's file,
//! a variant's base or a nested prefab's file, or reverted), override marks on fields and
//! components (a blue bar and blue label, Unity's bold-with-a-margin-line), the outliner's
//! colors and badges, and the bar shown while a prefab is open for editing.
//!
//! Buttons queue an [`Action`]; [`run_actions`] carries them out after the UI pass, so a
//! rebuild never happens while a panel is still drawing the entities it replaces.

use engine::prelude::*;
use serde_json::Value;

use crate::editor::Editor;
use crate::prefab::{self, Item, Override, PrefabInstance, PrefabPart, Target};
use crate::{inspector, scene_ops, theme};

/// Prefab copies' blue (Unity's).
pub const PREFAB: egui::Color32 = egui::Color32::from_rgb(0x5c, 0xa8, 0xff);
/// Parts of a copy, a little quieter.
pub const PREFAB_PART: egui::Color32 = egui::Color32::from_rgb(0x9c, 0xc6, 0xf5);
/// Things added to a copy.
pub const ADDED: egui::Color32 = egui::Color32::from_rgb(0x5f, 0xd0, 0x7a);

/// A prefab change asked for from the UI.
#[derive(Debug, Clone)]
pub enum Action {
    Apply(Entity, Item, Target),
    /// Revert one change, or everything with `None`.
    Revert(Entity, Option<Item>),
    ApplyAll(Entity),
    Enter(Entity),
    Exit,
    Save,
    Variant(Entity),
    Unpack(Entity),
}

pub fn run_actions(world: &mut World, editor: &mut Editor) {
    for action in std::mem::take(&mut editor.prefab_actions) {
        match action {
            Action::Apply(root, item, target) => {
                prefab::apply_item(world, editor, root, &item, &target)
            }
            Action::Revert(root, item) => prefab::revert(world, editor, root, item.as_ref()),
            Action::ApplyAll(top) => prefab::apply_all(world, editor, top),
            Action::Enter(e) => prefab::enter(world, editor, e),
            Action::Exit => prefab::exit(world, editor),
            Action::Save => prefab::save_focus(world, editor),
            Action::Variant(root) => prefab::save_variant(world, editor, root),
            Action::Unpack(root) => prefab::unpack(world, editor, root),
        }
    }
    // While a prefab is open, only its entities can be selected.
    let keep: Vec<Entity> = editor
        .selection
        .iter()
        .copied()
        .filter(|&e| prefab::in_focus(world, editor, e))
        .collect();
    editor.selection = keep;
}

/// What the inspector marks on one entity: the overrides of its part.
pub struct Marks {
    pub root: Entity,
    pub part: String,
    pub overrides: Vec<Override>,
}

impl Marks {
    /// Whether the field at `pointer` of `component` is overridden (or inside, or around,
    /// an overridden one).
    pub fn field(&self, component: &str, pointer: &str) -> bool {
        self.overrides.iter().any(|o| {
            o.component == component
                && (o.field.is_empty()
                    || o.field == pointer
                    || o.field.starts_with(&format!("{pointer}/"))
                    || pointer.starts_with(&format!("{}/", o.field)))
        })
    }

    /// Whether `component` was added to this copy (not in the file).
    pub fn added(&self, component: &str) -> bool {
        self.overrides
            .iter()
            .any(|o| o.component == component && o.field.is_empty() && !o.value.is_null())
    }

    pub fn changed(&self, component: &str) -> bool {
        self.overrides.iter().any(|o| o.component == component)
    }

    /// Components this copy removed.
    pub fn removed(&self) -> Vec<String> {
        self.overrides
            .iter()
            .filter(|o| o.field.is_empty() && o.value.is_null())
            .map(|o| o.component.clone())
            .collect()
    }
}

/// The overrides of `entity`'s part, when it is part of a placed copy (refreshed now, so a
/// field shows as changed the frame it is edited).
pub fn marks(world: &mut World, entity: Entity) -> Option<Marks> {
    let (root, part) = prefab::owner(world, entity)?;
    prefab::refresh(world, root);
    let link = world.get::<PrefabInstance>(root)?;
    Some(Marks {
        root,
        part: part.clone(),
        overrides: link
            .overrides
            .iter()
            .filter(|o| o.part == part)
            .cloned()
            .collect(),
    })
}

/// Paint the override bar at the left edge of the panel beside `row`.
pub fn bar(ui: &egui::Ui, row: egui::Rect) {
    let x = ui.clip_rect().left() + 1.0;
    ui.painter().rect_filled(
        egui::Rect::from_x_y_ranges(x..=x + 2.5, row.y_range()),
        1,
        PREFAB,
    );
}

/// The right-click menu of an overridden field or component: revert it, or apply it to one of
/// the files it can go to.
pub fn item_menu(
    ui: &mut egui::Ui,
    world: &mut World,
    editor: &mut Editor,
    root: Entity,
    item: Item,
) {
    if ui.button("↺ Revert").clicked() {
        editor
            .prefab_actions
            .push(Action::Revert(root, Some(item.clone())));
        ui.close();
    }
    let (part, component) = match &item {
        Item::Field {
            part, component, ..
        } => (part.clone(), component.clone()),
        Item::Removed(k) => (k.clone(), String::new()),
        Item::Added(e) => (
            world
                .get::<ChildOf>(*e)
                .and_then(|p| world.get::<PrefabPart>(p.parent()))
                .map(|p| p.key.clone())
                .unwrap_or_default(),
            "+".into(),
        ),
    };
    for target in prefab::targets(world, root, &part, &component) {
        if ui.button(apply_label(world, &target)).clicked() {
            editor
                .prefab_actions
                .push(Action::Apply(root, item.clone(), target));
            ui.close();
        }
    }
}

fn apply_label(world: &mut World, target: &Target) -> String {
    let name = prefab::display_name(&target.path);
    match prefab::base_of(world, &target.path) {
        Some(base) => format!(
            "✔ Apply to {name} (variant of {})",
            prefab::display_name(&base)
        ),
        None if target.prefix.is_empty() => format!("✔ Apply to {name}"),
        None => format!("✔ Apply to {name} (nested)"),
    }
}

/// The Details panel's prefab bar for `entity`.
pub fn details_bar(ui: &mut egui::Ui, world: &mut World, editor: &mut Editor, entity: Entity) {
    let Some((root, _)) = prefab::owner(world, entity) else {
        added_bar(ui, world, editor, entity);
        return;
    };
    // The prefab this entity belongs to directly: itself, or the nearest nested root above.
    let mut top = entity;
    while world.get::<PrefabInstance>(top).is_none() {
        match world.get::<ChildOf>(top) {
            Some(p) => top = p.parent(),
            None => break,
        }
    }
    let Some(path) = world.get::<PrefabInstance>(top).map(|l| l.path.clone()) else {
        return;
    };
    let editing = editor.prefab_mode.iter().any(|f| f.entity == top);
    let in_editing = editor
        .prefab_mode
        .iter()
        .any(|f| f.entity == root || prefab::owner(world, f.entity).is_some_and(|o| o.0 == root));
    let base = prefab::base_of(world, &path);
    egui::Frame::new()
        .fill(egui::Color32::from_rgba_unmultiplied(0x5c, 0xa8, 0xff, 22))
        .stroke(egui::Stroke::new(
            1.0,
            egui::Color32::from_rgba_unmultiplied(0x5c, 0xa8, 0xff, 90),
        ))
        .corner_radius(4)
        .inner_margin(egui::Margin::same(6))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(format!("📦 {}", prefab::display_name(&path)))
                        .color(PREFAB)
                        .strong(),
                )
                .on_hover_text(&path);
                let mut what = Vec::new();
                if let Some(base) = &base {
                    what.push(format!("variant of {}", prefab::display_name(base)));
                }
                if top != root
                    && let Some(outer) = world.get::<PrefabInstance>(root)
                {
                    what.push(format!("in {}", prefab::display_name(&outer.path)));
                }
                if top != entity {
                    what.push("part".into());
                }
                if !what.is_empty() {
                    ui.label(theme::dim(what.join(" · ")));
                }
            });
            let edit_button = |ui: &mut egui::Ui, editor: &mut Editor| {
                if !editing
                    && ui
                        .button("✏ Edit prefab")
                        .on_hover_text(
                            "Open the prefab in place: the rest of the scene can't be \
                             selected, this copy's own changes are put aside, and going back \
                             saves it to every copy",
                        )
                        .clicked()
                {
                    editor.prefab_actions.push(Action::Enter(top));
                }
            };
            if in_editing {
                ui.label(theme::dim(
                    "Editing the prefab: changes reach every copy when you save or go back",
                ));
                edit_button(ui, editor);
                return;
            }
            ui.horizontal_wrapped(|ui| {
                let link = world
                    .get::<PrefabInstance>(root)
                    .cloned()
                    .unwrap_or_default();
                let added = prefab::added_entities(world, root);
                let count = link.overrides.len() + link.removed.len() + added.len();
                let r = ui.menu_button(format!("Overrides ({count}) ⏷"), |ui| {
                    overrides_list(ui, world, editor, root, &link, &added);
                });
                editor.overrides_button = Some(r.response.rect);
                if ui
                    .add_enabled(count > 0, egui::Button::new("✔ Apply all"))
                    .on_hover_text(format!("Save every change to {path} and update all copies"))
                    .clicked()
                {
                    editor.prefab_actions.push(Action::ApplyAll(root));
                }
                if ui
                    .add_enabled(count > 0, egui::Button::new("↺ Revert all"))
                    .on_hover_text("Make this copy match its file again")
                    .clicked()
                {
                    editor.prefab_actions.push(Action::Revert(root, None));
                }
                ui.menu_button("More ⏷", |ui| {
                    if ui.button("Select the copy's root").clicked() {
                        editor.select(Some(root));
                        ui.close();
                    }
                    if top != root
                        && ui
                            .button(format!(
                                "Apply this nested copy to {}",
                                prefab::display_name(&path)
                            ))
                            .clicked()
                    {
                        editor.prefab_actions.push(Action::ApplyAll(top));
                        ui.close();
                    }
                    if ui
                        .button("Save as variant")
                        .on_hover_text(
                            "A new prefab based on this one, with this copy's changes; it \
                             still follows the original",
                        )
                        .clicked()
                    {
                        editor.prefab_actions.push(Action::Variant(root));
                        ui.close();
                    }
                    if ui
                        .button("Unpack")
                        .on_hover_text("Make this copy plain entities, no longer linked to a file")
                        .clicked()
                    {
                        editor.prefab_actions.push(Action::Unpack(root));
                        ui.close();
                    }
                });
                edit_button(ui, editor);
            });
        });
    ui.add_space(4.0);
}

/// An entity added under a copy: say so, and offer to apply or remove it.
fn added_bar(ui: &mut egui::Ui, world: &mut World, editor: &mut Editor, entity: Entity) {
    let Some(parent) = world.get::<ChildOf>(entity).map(|p| p.parent()) else {
        return;
    };
    let Some((root, _)) = prefab::owner(world, parent) else {
        return;
    };
    let Some(path) = world.get::<PrefabInstance>(root).map(|l| l.path.clone()) else {
        return;
    };
    ui.horizontal_wrapped(|ui| {
        ui.label(
            egui::RichText::new(format!("+ Added to {}", prefab::display_name(&path))).color(ADDED),
        );
        ui.menu_button("Apply / remove ⏷", |ui| {
            item_menu(ui, world, editor, root, Item::Added(entity));
        });
    });
    ui.add_space(4.0);
}

/// The Overrides list: every change of a copy beside its file's value, each with revert and
/// apply.
fn overrides_list(
    ui: &mut egui::Ui,
    world: &mut World,
    editor: &mut Editor,
    root: Entity,
    link: &PrefabInstance,
    added: &[Entity],
) {
    ui.set_min_width(560.0);
    let base = prefab::with_files(world, |files, _| files.expand(&link.path).ok());
    let parts = prefab::part_entities(world, root);
    let part_name = |world: &World, key: &str| match parts.get(key) {
        Some(&e) if key.is_empty() => format!("{} (root)", scene_ops::label(world, e)),
        Some(&e) => scene_ops::label(world, e),
        None => key.to_string(),
    };
    if link.overrides.is_empty() && link.removed.is_empty() && added.is_empty() {
        ui.label(theme::dim("No changes: this copy matches its file"));
        return;
    }
    egui::ScrollArea::vertical()
        .max_height(360.0)
        .show(ui, |ui| {
            let mut last_part = None;
            for o in &link.overrides {
                if last_part != Some(&o.part) {
                    last_part = Some(&o.part);
                    ui.add_space(3.0);
                    ui.label(egui::RichText::new(part_name(world, &o.part)).strong());
                }
                let was = base
                    .as_ref()
                    .and_then(|b| prefab::find(b, &o.part))
                    .and_then(|p| prefab::value_at(p, &o.component, &o.field));
                let what = if o.field.is_empty() {
                    match (&was, o.value.is_null()) {
                        (_, true) => format!("{} removed", o.component),
                        (None, _) | (Some(Value::Null), _) => format!("{} added", o.component),
                        _ => o.component.clone(),
                    }
                } else {
                    format!("{} › {}", o.component, field_name(&o.field))
                };
                let item = Item::Field {
                    part: o.part.clone(),
                    component: o.component.clone(),
                    field: o.field.clone(),
                };
                row(ui, world, editor, root, item, &what, |ui| {
                    if !o.value.is_null() && !(o.field.is_empty() && was.is_none()) {
                        ui.label(theme::dim(short(was.as_ref().unwrap_or(&Value::Null))));
                        ui.label(theme::dim("to"));
                        ui.label(egui::RichText::new(short(&o.value)).color(PREFAB));
                    }
                });
            }
            if !link.removed.is_empty() {
                ui.add_space(3.0);
                ui.label(egui::RichText::new("Removed").strong());
                for key in &link.removed {
                    let name = base
                        .as_ref()
                        .and_then(|b| prefab::find(b, key))
                        .and_then(|p| p.name.clone())
                        .unwrap_or_else(|| key.clone());
                    row(
                        ui,
                        world,
                        editor,
                        root,
                        Item::Removed(key.clone()),
                        &format!("− {name}"),
                        |_| {},
                    );
                }
            }
            if !added.is_empty() {
                ui.add_space(3.0);
                ui.label(egui::RichText::new("Added").strong());
                for &e in added {
                    let name = format!("+ {}", scene_ops::label(world, e));
                    row(ui, world, editor, root, Item::Added(e), &name, |_| {});
                }
            }
        });
    ui.separator();
    ui.horizontal(|ui| {
        if ui.button("↺ Revert all").clicked() {
            editor.prefab_actions.push(Action::Revert(root, None));
            ui.close();
        }
        if ui.button("✔ Apply all").clicked() {
            editor.prefab_actions.push(Action::ApplyAll(root));
            ui.close();
        }
    });
}

fn row(
    ui: &mut egui::Ui,
    world: &mut World,
    editor: &mut Editor,
    root: Entity,
    item: Item,
    what: &str,
    values: impl FnOnce(&mut egui::Ui),
) {
    ui.horizontal(|ui| {
        ui.add_space(10.0);
        ui.label(what);
        values(ui);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.menu_button("✔ ⏷", |ui| {
                item_menu(ui, world, editor, root, item.clone())
            })
            .response
            .on_hover_text("Apply to a prefab file");
            if ui.small_button("↺").on_hover_text("Revert").clicked() {
                editor
                    .prefab_actions
                    .push(Action::Revert(root, Some(item.clone())));
            }
        });
    });
}

/// "/material/inline/emissive" → "Material › Emissive" (enum variant names left out).
fn field_name(pointer: &str) -> String {
    pointer
        .split('/')
        .filter(|s| !s.is_empty() && !matches!(*s, "inline" | "asset" | "shape" | "handle"))
        .map(inspector::pretty)
        .collect::<Vec<_>>()
        .join(" › ")
}

/// A value in a few characters.
pub fn short(v: &Value) -> String {
    let s = match v {
        Value::Number(n) => match n.as_f64() {
            Some(f) if n.is_f64() => format!("{f:.2}"),
            _ => n.to_string(),
        },
        Value::Array(items) if items.iter().all(Value::is_number) => format!(
            "[{}]",
            items.iter().map(short).collect::<Vec<_>>().join(", ")
        ),
        Value::String(s) => s.clone(),
        Value::Null => "none".into(),
        Value::Object(_) | Value::Array(_) => "{…}".into(),
        Value::Bool(b) => b.to_string(),
    };
    if s.chars().count() > 28 {
        format!("{}…", s.chars().take(27).collect::<String>())
    } else {
        s
    }
}

/// How an outliner row shows a prefab: text color, a badge, and whether it carries overrides.
pub struct RowStyle {
    pub color: Option<egui::Color32>,
    pub icon: Option<&'static str>,
    pub added: bool,
    pub changed: bool,
}

pub fn row_style(world: &World, entity: Entity) -> RowStyle {
    let mut style = RowStyle {
        color: None,
        icon: None,
        added: false,
        changed: false,
    };
    if world.get::<PrefabInstance>(entity).is_some() {
        style.icon = Some("📦");
    }
    match prefab::owner(world, entity) {
        Some((root, key)) => {
            style.color = Some(if key.is_empty() { PREFAB } else { PREFAB_PART });
            style.changed = world
                .get::<PrefabInstance>(root)
                .is_some_and(|l| l.overrides.iter().any(|o| o.part == key));
        }
        None => {
            style.added = world
                .get::<ChildOf>(entity)
                .is_some_and(|p| prefab::owner(world, p.parent()).is_some());
        }
    }
    style
}

/// The bar across the scene view while a prefab is open: where you are, save, back.
pub fn editing_bar(ctx: &egui::Context, world: &mut World, editor: &mut Editor, rect: egui::Rect) {
    if editor.prefab_mode.is_empty() {
        return;
    }
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Background,
        egui::Id::new("prefab frame"),
    ));
    painter.rect_stroke(
        rect.shrink(1.0),
        0,
        egui::Stroke::new(2.0, PREFAB),
        egui::StrokeKind::Inside,
    );
    let changed = editor
        .prefab_mode
        .last()
        .cloned()
        .is_some_and(|f| prefab::focus_changed(world, &f));
    egui::Area::new(egui::Id::new("prefab editing bar"))
        .fixed_pos(rect.min + egui::vec2(8.0, 46.0))
        .order(egui::Order::Middle)
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(egui::Color32::from_rgba_unmultiplied(18, 32, 52, 240))
                .stroke(egui::Stroke::new(1.0, PREFAB))
                .corner_radius(5)
                .inner_margin(egui::Margin::symmetric(8, 5))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        if ui.link("🏠 Scene").clicked() {
                            for _ in 0..editor.prefab_mode.len() {
                                editor.prefab_actions.push(Action::Exit);
                            }
                        }
                        let depth = editor.prefab_mode.len();
                        for (i, f) in editor.prefab_mode.clone().iter().enumerate() {
                            ui.label(theme::dim("›"));
                            let name = format!("📦 {}", prefab::display_name(&f.path));
                            if i + 1 == depth {
                                ui.label(egui::RichText::new(name).color(PREFAB).strong());
                            } else if ui.link(name).clicked() {
                                for _ in i + 1..depth {
                                    editor.prefab_actions.push(Action::Exit);
                                }
                            }
                        }
                        ui.add_space(12.0);
                        ui.label(theme::dim(if changed {
                            "unsaved changes"
                        } else {
                            "saved"
                        }));
                        if ui
                            .add_enabled(changed, egui::Button::new("💾 Save"))
                            .on_hover_text("Save the prefab and update every copy (Ctrl+S)")
                            .clicked()
                        {
                            editor.prefab_actions.push(Action::Save);
                        }
                        if ui
                            .button("⬅ Back")
                            .on_hover_text("Save and go back to the scene")
                            .clicked()
                        {
                            editor.prefab_actions.push(Action::Exit);
                        }
                    });
                });
        });
}
