//! The scatter brush: paint copies of models onto any surface (Unreal's Foliage mode, Unity's
//! Polybrush Scatter Prefabs and terrain tree painting).
//!
//! Pick models for the palette, then drag over the scene: each dab fills the brush circle up to
//! the density (copies per 100 m²), so going over a spot again doesn't pile copies up. Every copy
//! is dropped onto whatever is below it (meshes or landscape, never other painted copies) with
//! its pivot on the surface, a random size from the scale range, a random turn, optionally
//! tilted to the slope, and nothing is placed on slopes steeper than the limit. Shift erases.
//! One stroke is one undo step. Copies are ordinary placed models under one "Scatter: <model>"
//! group per model, so scenes save them like anything else.

use engine::debug::DebugDraw;
use engine::prelude::*;

use crate::editor::{self, Editor};
use crate::scene_ops::{self, InScene, SceneOrder};
use crate::undo::Command;
use crate::view::ViewInfo;

/// Marks the group entity that holds the painted copies of `model`.
#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ScatterGroup {
    pub model: String,
}

/// Seconds between dabs while the button is held.
const DAB_SECS: f32 = 0.12;
/// Most copies one dab adds.
const MAX_PER_DAB: usize = 40;
/// How far new copies stay from other models' copies, meters.
const CLEARANCE: f64 = 0.6;

/// The brush's settings and the stroke being painted.
pub struct Scatter {
    /// Model keys to paint with (picked in turn at random).
    pub palette: Vec<String>,
    pub radius: f32,
    /// Copies per 100 m² (a 10 m square).
    pub density: f32,
    /// Size range, as factors of the model's own size.
    pub scale: [f32; 2],
    pub random_turn: bool,
    /// Tilt copies to the surface's slope (rocks) instead of standing them upright (trees).
    pub align: bool,
    /// Steepest slope that gets copies, degrees.
    pub max_slope: f32,
    /// Push copies this far into the surface, meters (so rocks don't float on bumps).
    pub sink: f32,
    /// Typed into the palette's search box.
    pub filter: String,
    since: f32,
    stroke: Vec<Command>,
    added: usize,
    removed: usize,
    rng: u64,
}

impl Default for Scatter {
    fn default() -> Self {
        Self {
            palette: Vec::new(),
            radius: 4.0,
            density: 12.0,
            scale: [0.8, 1.25],
            random_turn: true,
            align: false,
            max_slope: 35.0,
            sink: 0.05,
            filter: String::new(),
            since: f32::INFINITY,
            stroke: Vec::new(),
            added: 0,
            removed: 0,
            rng: 0x9e37_79b9_7f4a_7c15,
        }
    }
}

impl Scatter {
    /// A random number in 0..1 (xorshift; strokes repeat from the same start).
    fn random(&mut self) -> f32 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        (self.rng >> 40) as f32 / (1u64 << 24) as f32
    }
}

/// A painted copy: the copy, its group, the group's model and where it stands.
struct PaintedCopy {
    entity: Entity,
    group: Entity,
    model: String,
    at: DVec3,
}

/// The painted copies in the scene.
fn copies(world: &mut World) -> Vec<PaintedCopy> {
    let mut q = world.query::<(Entity, &ScatterGroup, &Children)>();
    let groups: Vec<(Entity, String, Vec<Entity>)> = q
        .iter(world)
        .map(|(e, g, c)| (e, g.model.clone(), c.iter().collect()))
        .collect();
    let mut out = Vec::new();
    for (group, model, children) in groups {
        for entity in children {
            if let Some(t) = world.get::<Transform>(entity) {
                out.push(PaintedCopy {
                    entity,
                    group,
                    model: model.clone(),
                    at: t.translation,
                });
            }
        }
    }
    out
}

/// Whether `entity` is a scatter group or anything painted into one.
pub fn painted(world: &World, mut entity: Entity) -> bool {
    loop {
        if world.get::<ScatterGroup>(entity).is_some() {
            return true;
        }
        match world.get::<ChildOf>(entity) {
            Some(p) => entity = p.parent(),
            None => return false,
        }
    }
}

/// The surface a ray meets (meshes or landscape, skipping painted copies and editor helpers)
/// and its normal, from two neighbouring rays.
pub fn surface(world: &mut World, origin: DVec3, dir: Vec3) -> Option<(DVec3, Vec3)> {
    let skip_painted: std::collections::HashSet<Entity> = copies(world)
        .into_iter()
        .flat_map(|c| {
            let mut all = vec![c.entity];
            let mut stack = vec![c.entity];
            while let Some(e) = stack.pop() {
                if let Some(children) = world.get::<Children>(e) {
                    all.extend(children.iter());
                    stack.extend(children.iter());
                }
            }
            all
        })
        .collect();
    crate::picking::surface_and_normal(world, origin, dir, &|e| skip_painted.contains(&e))
}

/// The group holding `model`'s copies, made (and recorded in the stroke) when missing.
fn group_for(world: &mut World, editor: &mut Editor, model: &str) -> Entity {
    let mut q = world.query_filtered::<(Entity, &ScatterGroup), With<InScene>>();
    if let Some(e) = q
        .iter(world)
        .find(|(_, g)| g.model == model)
        .map(|(e, _)| e)
    {
        return e;
    }
    let e = world
        .spawn((
            Transform::IDENTITY,
            Name::new(format!("Scatter: {}", crate::models::display_name(model))),
            ScatterGroup {
                model: model.to_string(),
            },
            SceneOrder::next(),
            InScene,
        ))
        .id();
    editor.scatter.stroke.push(Command::Spawned {
        saved: scene_ops::save_entity(world, e),
        parent: None,
    });
    e
}

/// One dab at `center`: fill the circle up to the density, or erase what's in it.
pub fn dab(world: &mut World, editor: &mut Editor, center: DVec3, erase: bool) {
    let radius = editor.scatter.radius as f64;
    let inside = |p: DVec3| {
        let d = p - center;
        d.x * d.x + d.z * d.z <= radius * radius
    };
    let existing = copies(world);
    if erase {
        // With models picked, erase only those (Unreal's per foliage type); else everything.
        let palette = &editor.scatter.palette;
        let doomed: Vec<PaintedCopy> = existing
            .into_iter()
            .filter(|c| inside(c.at) && (palette.is_empty() || palette.contains(&c.model)))
            .collect();
        for c in doomed {
            let saved = scene_ops::save_entity(world, c.entity);
            world.entity_mut(c.entity).despawn();
            editor.scatter.stroke.push(Command::Despawned {
                saved,
                parent: Some(c.group),
            });
            editor.scatter.removed += 1;
        }
        return;
    }
    if editor.scatter.palette.is_empty() {
        return;
    }
    let per_m2 = (editor.scatter.density / 100.0).max(1e-4) as f64;
    let area = std::f64::consts::PI * radius * radius;
    // Density counts only the picked models' copies, so trees can be painted among rocks;
    // other copies just keep a small clearance.
    let near = |c: &&PaintedCopy| (c.at - center).length() < radius * 1.5;
    let mut taken: Vec<DVec3> = existing
        .iter()
        .filter(near)
        .filter(|c| editor.scatter.palette.contains(&c.model))
        .map(|c| c.at)
        .collect();
    let others: Vec<DVec3> = existing
        .iter()
        .filter(near)
        .filter(|c| !editor.scatter.palette.contains(&c.model))
        .map(|c| c.at)
        .collect();
    let have = taken.iter().filter(|p| inside(**p)).count();
    let want = ((per_m2 * area).round() as usize)
        .saturating_sub(have)
        .min(MAX_PER_DAB);
    // Copies keep half the average spacing between each other.
    let spacing = 0.5 / per_m2.sqrt();
    let mut added = 0;
    for _ in 0..want * 6 {
        if added >= want {
            break;
        }
        let s = &mut editor.scatter;
        let (r, a) = (
            s.random().sqrt() as f64 * radius,
            s.random() as f64 * std::f64::consts::TAU,
        );
        let at = center + DVec3::new(r * a.cos(), 0.0, r * a.sin());
        let close = |p: &DVec3, gap: f64| {
            let d = *p - at;
            d.x * d.x + d.z * d.z < gap * gap
        };
        if taken.iter().any(|p| close(p, spacing)) || others.iter().any(|p| close(p, CLEARANCE)) {
            continue;
        }
        let from = at + DVec3::Y * (radius + 4.0);
        let Some((hit, normal)) = surface(world, from, Vec3::NEG_Y) else {
            continue;
        };
        if normal.y.clamp(-1.0, 1.0).acos().to_degrees() > editor.scatter.max_slope {
            continue;
        }
        let s = &mut editor.scatter;
        let pick = (s.random() * s.palette.len() as f32) as usize % s.palette.len();
        let model = s.palette[pick].clone();
        let size = s.scale[0] + (s.scale[1] - s.scale[0]) * s.random();
        let turn = if s.random_turn {
            Quat::from_rotation_y(s.random() * std::f32::consts::TAU)
        } else {
            Quat::IDENTITY
        };
        let tilt = if s.align {
            Quat::from_rotation_arc(Vec3::Y, normal)
        } else {
            Quat::IDENTITY
        };
        let sink = s.sink as f64;
        let transform = Transform::from_translation(hit - normal.as_dvec3() * sink)
            .with_rotation(tilt * turn)
            .with_scale(Vec3::splat(size));
        let group = group_for(world, editor, &model);
        match crate::models::spawn_copy(world, &model, transform, Some(group)) {
            Ok(copy) => {
                editor.scatter.stroke.push(Command::Spawned {
                    saved: scene_ops::save_entity(world, copy),
                    parent: Some(group),
                });
                taken.push(hit);
                added += 1;
            }
            Err(e) => {
                let t = editor::now(world);
                editor.error(t, format!("Could not paint {model}: {e}"));
                editor.scatter.palette.retain(|k| *k != model);
                return;
            }
        }
    }
    editor.scatter.added += added;
}

/// Record the stroke as one undo step.
pub fn end_stroke(world: &mut World, editor: &mut Editor) {
    let s = &mut editor.scatter;
    if s.stroke.is_empty() {
        return;
    }
    let commands = std::mem::take(&mut s.stroke);
    let label = match (s.added, s.removed) {
        (a, 0) => format!("Scatter {a} copies"),
        (0, r) => format!("Erase {r} copies"),
        (a, r) => format!("Scatter {a}, erase {r}"),
    };
    s.added = 0;
    s.removed = 0;
    let t = editor::now(world);
    editor
        .history
        .record(world, Command::Group(label.clone(), commands), t);
    editor.info(t, label);
}

/// The brush in the scene view: its circle on the surface under the cursor, dabs while the
/// left button is held (Shift erases).
pub fn brush(
    ui: &egui::Ui,
    world: &mut World,
    editor: &mut Editor,
    view: &ViewInfo,
    response: &egui::Response,
) {
    let held = response.is_pointer_button_down_on() && ui.input(|i| i.pointer.primary_down());
    if !held {
        editor.scatter.since = f32::INFINITY;
        end_stroke(world, editor);
    }
    let Some(pos) = response.hover_pos().or(response.interact_pointer_pos()) else {
        return;
    };
    let (origin, dir) = view.ray(pos);
    let Some((hit, normal)) = surface(world, origin, dir) else {
        return;
    };
    let erase = ui.input(|i| i.modifiers.shift);
    draw_circle(world, editor, hit, normal, erase);
    if !held {
        return;
    }
    editor.scatter.since += ui.input(|i| i.stable_dt);
    if editor.scatter.since < DAB_SECS {
        return;
    }
    editor.scatter.since = 0.0;
    dab(world, editor, hit, erase);
}

/// The brush circle at `at` (green paints, red erases).
pub fn draw_circle(world: &mut World, editor: &Editor, at: DVec3, normal: Vec3, erase: bool) {
    let color = if erase {
        [0.95, 0.4, 0.35, 0.95]
    } else {
        [0.45, 0.95, 0.5, 0.95]
    };
    let mut debug = world.resource_mut::<DebugDraw>();
    let lift = normal.as_dvec3() * 0.05;
    debug
        .circle(at + lift, normal, editor.scatter.radius, color)
        .on_top();
    debug
        .circle(
            at + lift,
            normal,
            editor.scatter.radius * 0.04,
            [color[0], color[1], color[2], 0.6],
        )
        .on_top();
}

/// The brush panel, floating in the scene view while scattering.
pub fn panel(ui: &mut egui::Ui, world: &mut World, editor: &mut Editor) {
    let filter = editor.scatter.filter.to_lowercase();
    let mut models: Vec<String> = crate::models::cached(world, editor)
        .iter()
        .filter(|k| !crate::materials::is_material(k))
        .filter(|k| filter.is_empty() || k.to_lowercase().contains(&filter))
        .cloned()
        .collect();
    // Animation files have nothing to paint; picked models come first.
    models.retain(|k| !crate::rigs::is_clip_file(world, k));
    models.sort_by_key(|k| !editor.scatter.palette.contains(k));
    ui.horizontal(|ui| {
        ui.label(crate::theme::dim("Paint with (click to pick)"));
        ui.add(
            egui::TextEdit::singleline(&mut editor.scatter.filter)
                .hint_text("🔍 Search")
                .desired_width(110.0),
        );
    });
    egui::ScrollArea::vertical()
        .max_height(110.0)
        .id_salt("scatter palette")
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                for key in &models {
                    let on = editor.scatter.palette.contains(key);
                    if ui
                        .selectable_label(on, crate::models::display_name(key))
                        .on_hover_text(key)
                        .clicked()
                    {
                        if on {
                            editor.scatter.palette.retain(|k| k != key);
                        } else {
                            editor.scatter.palette.push(key.clone());
                        }
                    }
                }
            });
        });
    let s = &mut editor.scatter;
    egui::Grid::new("scatter").num_columns(2).show(ui, |ui| {
        ui.label("Radius");
        ui.add(
            egui::Slider::new(&mut s.radius, 0.5..=40.0)
                .logarithmic(true)
                .suffix(" m"),
        );
        ui.end_row();
        ui.label("Density");
        ui.add(
            egui::Slider::new(&mut s.density, 0.2..=400.0)
                .logarithmic(true)
                .suffix(" / 100 m²"),
        );
        ui.end_row();
        ui.label("Size");
        let [mut low, mut high] = s.scale;
        ui.horizontal(|ui| {
            ui.add(
                egui::DragValue::new(&mut low)
                    .speed(0.01)
                    .range(0.05..=high),
            );
            ui.label("to");
            ui.add(
                egui::DragValue::new(&mut high)
                    .speed(0.01)
                    .range(low..=20.0),
            );
        });
        s.scale = [low, high];
        ui.end_row();
        ui.label("Steepest");
        ui.add(egui::Slider::new(&mut s.max_slope, 0.0..=90.0).suffix("°"));
        ui.end_row();
        ui.label("Sink");
        ui.add(egui::Slider::new(&mut s.sink, 0.0..=1.0).suffix(" m"));
        ui.end_row();
    });
    ui.horizontal(|ui| {
        ui.checkbox(&mut s.random_turn, "Random turn");
        ui.checkbox(&mut s.align, "Tilt to slope");
    });
    let painted = copies(world).len();
    ui.label(crate::theme::dim(format!(
        "{painted} painted · drag to paint · Shift erases the picked models · Ctrl+Z undoes a stroke"
    )));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn random_numbers_stay_in_range_and_spread() {
        let mut s = Scatter::default();
        let values: Vec<f32> = (0..1000).map(|_| s.random()).collect();
        assert!(values.iter().all(|v| (0.0..1.0).contains(v)));
        let low = values.iter().filter(|v| **v < 0.5).count();
        assert!((400..600).contains(&low), "{low} of 1000 below a half");
    }
}
