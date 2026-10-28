//! The Property Matrix, as Unreal's: the selected objects as rows and one component's fields as
//! columns, so many objects are compared and set one by one in a table (Details sets one value
//! on all of them). Click a column's name to sort by it, a row's name to select that object.

use engine::core::registry::TypeRegistry;
use engine::prelude::*;
use serde_json::Value;

use crate::editor::{self, Editor};
use crate::theme;
use crate::undo::Command;

/// Columns a table shows at most (a component with hundreds of fields stays readable).
const MAX_COLUMNS: usize = 24;

#[derive(Default)]
pub struct Matrix {
    /// The component whose fields are the columns.
    pub component: Option<String>,
    /// Sorted by this column, ascending or not.
    pub sort: Option<(String, bool)>,
}

/// The editable leaves of `value`: numbers, booleans and strings, as `/a/0` paths, in order.
pub fn columns(value: &Value) -> Vec<String> {
    fn walk(v: &Value, path: String, out: &mut Vec<String>) {
        match v {
            Value::Number(_) | Value::Bool(_) | Value::String(_) => out.push(path),
            Value::Array(items) if items.len() <= 4 => {
                for (i, item) in items.iter().enumerate() {
                    walk(item, format!("{path}/{i}"), out);
                }
            }
            Value::Object(map) => {
                for (k, item) in map {
                    walk(item, format!("{path}/{k}"), out);
                }
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    walk(value, String::new(), &mut out);
    out
}

/// "/translation/0" → "Translation X", "/light/range" → "Light range".
pub fn column_name(path: &str) -> String {
    let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    let words: Vec<String> = parts
        .iter()
        .enumerate()
        .map(|(n, p)| match p.parse::<usize>() {
            Ok(i) => {
                // A color's channels, a vector's axes.
                let color = n > 0 && parts[n - 1].contains("color");
                let names = if color {
                    ["R", "G", "B", "A"]
                } else {
                    ["X", "Y", "Z", "W"]
                };
                names.get(i).map_or(p.to_string(), |a| a.to_string())
            }
            Err(_) => p.replace('_', " "),
        })
        .collect();
    crate::inspector::pretty(&words.join(" "))
}

/// Order for sorting a column: numbers by value, then the rest by text.
fn sort_key(v: Option<&Value>) -> (f64, String) {
    match v {
        Some(Value::Number(n)) => (n.as_f64().unwrap_or(0.0), String::new()),
        Some(Value::Bool(b)) => (*b as u8 as f64, String::new()),
        Some(other) => (f64::INFINITY, other.to_string()),
        None => (f64::INFINITY, String::new()),
    }
}

pub fn panel(ui: &mut egui::Ui, world: &mut World, editor: &mut Editor) {
    let rows: Vec<Entity> = editor
        .selection
        .iter()
        .copied()
        .filter(|&e| world.get::<crate::scene_ops::InScene>(e).is_some())
        .collect();
    if rows.is_empty() {
        ui.add_space(16.0);
        ui.vertical_centered(|ui| {
            ui.label(theme::dim(
                "Select objects to compare them here: one row each, a column per field",
            ));
        });
        return;
    }
    let registry = world.resource::<TypeRegistry>().clone();
    let shared: Vec<String> = registry
        .components()
        .filter(|c| c.name != "Name" && rows.iter().all(|&e| (c.has)(world, e)))
        .map(|c| c.name.clone())
        .collect();
    let m = &mut editor.matrix;
    if m.component.as_ref().is_none_or(|c| !shared.contains(c)) {
        // Where things are is what's compared most.
        m.component = shared
            .iter()
            .find(|c| *c == "Transform")
            .or(shared.first())
            .cloned();
        m.sort = None;
    }
    ui.horizontal(|ui| {
        ui.label("Component");
        egui::ComboBox::from_id_salt("matrix component")
            .selected_text(m.component.clone().unwrap_or_default())
            .show_ui(ui, |ui| {
                for name in &shared {
                    if ui
                        .selectable_label(m.component.as_ref() == Some(name), name)
                        .clicked()
                    {
                        m.component = Some(name.clone());
                        m.sort = None;
                    }
                }
            });
        ui.label(theme::dim(format!(
            "{} object{} · click a column to sort, a name to select",
            rows.len(),
            if rows.len() == 1 { "" } else { "s" }
        )));
    });
    let Some(entry) = m
        .component
        .as_ref()
        .and_then(|c| registry.component(c))
        .cloned()
    else {
        return;
    };
    let mut table: Vec<(Entity, Value)> = rows
        .iter()
        .filter_map(|&e| Some((e, (entry.get)(world, e)?)))
        .collect();
    let Some(first) = table.first().map(|r| r.1.clone()) else {
        return;
    };
    let mut cols = columns(&first);
    // A quaternion reads as nothing to people; the gizmo and Details turn things.
    if entry.name == "Transform" {
        cols.retain(|c| !c.starts_with("/rotation"));
    }
    let hidden = cols.len().saturating_sub(MAX_COLUMNS);
    cols.truncate(MAX_COLUMNS);
    if let Some((col, up)) = &m.sort {
        table.sort_by(|a, b| {
            let (ka, kb) = (sort_key(a.1.pointer(col)), sort_key(b.1.pointer(col)));
            let o = ka.0.total_cmp(&kb.0).then_with(|| ka.1.cmp(&kb.1));
            if *up { o } else { o.reverse() }
        });
    }

    let sorted = m.sort.clone();
    let primary_now = editor.primary();
    let mut edits: Vec<(Entity, Value, Value)> = Vec::new();
    let mut select = None;
    let mut sort_by = None;
    egui::ScrollArea::both().auto_shrink(false).show(ui, |ui| {
        egui::Grid::new("property matrix")
            .striped(true)
            .min_col_width(56.0)
            .spacing([10.0, 4.0])
            .show(ui, |ui| {
                ui.label(egui::RichText::new("Object").strong());
                for col in &cols {
                    let arrow = match &sorted {
                        Some((c, true)) if c == col => " ⏶",
                        Some((c, false)) if c == col => " ⏷",
                        _ => "",
                    };
                    if ui
                        .add(
                            egui::Label::new(
                                egui::RichText::new(format!("{}{arrow}", column_name(col)))
                                    .strong(),
                            )
                            .sense(egui::Sense::click()),
                        )
                        .on_hover_text(format!("{}{col}: click to sort", entry.name))
                        .clicked()
                    {
                        sort_by = Some(col.clone());
                    }
                }
                ui.end_row();
                for (e, value) in &table {
                    let name = crate::scene_ops::label(world, *e);
                    let primary = primary_now == Some(*e);
                    let text = egui::RichText::new(name);
                    if ui
                        .selectable_label(primary, if primary { text.strong() } else { text })
                        .clicked()
                    {
                        select = Some(*e);
                    }
                    let mut after = value.clone();
                    for col in &cols {
                        let Some(cell) = after.pointer_mut(col) else {
                            ui.label("");
                            continue;
                        };
                        match cell {
                            Value::Number(n) => {
                                let mut v = n.as_f64().unwrap_or(0.0);
                                let speed = (v.abs() * 0.01).clamp(0.001, 1000.0);
                                if ui
                                    .add(egui::DragValue::new(&mut v).speed(speed).max_decimals(3))
                                    .changed()
                                {
                                    // Integers stay integers.
                                    *n = if n.is_f64() {
                                        serde_json::Number::from_f64(v).unwrap_or(n.clone())
                                    } else {
                                        serde_json::Number::from(v.round() as i64)
                                    };
                                }
                            }
                            Value::Bool(b) => {
                                ui.checkbox(b, "");
                            }
                            Value::String(s) => {
                                ui.label(s.as_str());
                            }
                            _ => {
                                ui.label("");
                            }
                        }
                    }
                    if after != *value {
                        edits.push((*e, value.clone(), after));
                    }
                    ui.end_row();
                }
            });
        if hidden > 0 {
            ui.label(theme::dim(format!(
                "{hidden} more field{} not shown",
                if hidden == 1 { "" } else { "s" }
            )));
        }
    });
    if let Some(col) = sort_by {
        let m = &mut editor.matrix;
        m.sort = match &m.sort {
            Some((c, true)) if *c == col => Some((col, false)),
            Some((c, false)) if *c == col => None,
            _ => Some((col, true)),
        };
    }
    if let Some(e) = select {
        editor.select(Some(e));
        editor.selection = rows.clone();
        // The clicked one becomes the primary (Details shows it), the rest stay selected.
        editor.selection.retain(|&o| o != e);
        editor.selection.push(e);
    }
    for (e, before, after) in edits {
        editor::apply(
            world,
            editor,
            Command::Component {
                entity: e,
                name: entry.name.clone(),
                before: Some(before),
                after: Some(after),
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn columns_are_the_editable_leaves() {
        let v = json!({
            "translation": [1.0, 2.0, 3.0],
            "light": {"range": 10.0, "on": true, "name": "lamp"},
            "points": [1, 2, 3, 4, 5, 6],
        });
        let cols = columns(&v);
        assert!(cols.contains(&"/translation/2".to_string()));
        assert!(cols.contains(&"/light/on".to_string()));
        assert!(!cols.iter().any(|c| c.starts_with("/points")), "{cols:?}");
        assert_eq!(column_name("/translation/0"), "Translation X");
        assert_eq!(column_name("/light/cast_shadows"), "Light cast shadows");
        assert_eq!(column_name("/inline/base_color/3"), "Inline base color A");
    }
}
