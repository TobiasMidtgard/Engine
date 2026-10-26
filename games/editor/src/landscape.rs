//! Landscapes: a terrain under the scene that the editor sculpts with a brush.
//!
//! The landscape is a `PlanetTerrain` in its flat form, with a level pad at y = 0 under the
//! scene and hills around it. Brush dabs (raise, lower, restore, paint) are kept in its
//! [`Sculpt`] component, so they save with the scene and undo like any other edit, and are
//! sent to the terrain through the world's `TerrainEdit` contract. Undoing a dab rebuilds the
//! terrain from the remaining ones, because terrain edits can only be added.

use engine::prelude::*;
use engine::world::{EditOp, EditShape, Ground, SurfaceMaterials, TerrainEdit};
use engine_terrain::{FlatPad, PlanetTerrain};
use serde_json::json;

use crate::editor::{self, Editor, now};
use crate::scene_ops::InScene;
use crate::undo::Command;
use crate::view::ViewInfo;

/// Ground materials the brush paints, as the terrain names them.
pub const PAINTS: [&str; 8] = [
    "grass", "dirt", "mud", "sand", "rock", "snow", "leaves", "concrete",
];

/// What one brush dab does.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub enum SculptOp {
    #[default]
    Raise,
    Lower,
    /// Ease the ground back toward its natural shape (undoes sculpting where it is brushed).
    Restore,
    /// Paint a ground material (one of [`PAINTS`]).
    Paint(String),
}

/// One brush dab on the landscape.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Dab {
    /// Where the brush touched the ground (world meters).
    pub at: [f64; 3],
    /// Brush radius, meters.
    pub radius: f32,
    /// Raise and lower: meters per dab. Restore: strength 0..1.
    pub amount: f32,
    pub op: SculptOp,
}

/// The landscape's brush dabs, oldest first.
#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Sculpt {
    pub dabs: Vec<Dab>,
}

/// How many of [`Sculpt::dabs`] the terrain has been sent, and a terrain set aside while it
/// is rebuilt after an undo. Runtime only.
#[derive(Component, Default)]
pub struct SculptApplied {
    sent: usize,
    rebuilding: Option<PlanetTerrain>,
}

/// The brush settings in the editor.
#[derive(Debug, Clone)]
pub struct Brush {
    pub op: SculptOp,
    pub radius: f32,
    pub strength: f32,
    pub paint: usize,
    /// Seconds since the last dab while the button is held.
    pub since: f32,
}

impl Default for Brush {
    fn default() -> Self {
        Self {
            op: SculptOp::Raise,
            radius: 6.0,
            strength: 0.5,
            paint: 0,
            since: f32::INFINITY,
        }
    }
}

/// Raise and lower spheres are this many brush radii across.
const SPHERE_PER_RADIUS: f64 = 1.5;

/// Seconds between dabs while the brush is held.
const DAB_SECS: f32 = 0.06;

/// The scene's landscape, if it has one.
pub fn find(world: &mut World) -> Option<Entity> {
    let mut q = world.query_filtered::<Entity, (With<Sculpt>, With<InScene>)>();
    q.iter(world).next()
}

/// The terrain a new landscape starts from: 4 km of rolling country with a level pad 120 m
/// across at y = 0 under the scene.
fn new_terrain() -> (PlanetTerrain, Transform) {
    let pad = FlatPad {
        radius_m: 120.0,
        blend_m: 400.0,
        height_m: 40.0,
        mountains: 0.25,
        mountain_distance_m: 1_800.0,
        sea_distance_m: 0.0,
        ..FlatPad::default()
    };
    let terrain = PlanetTerrain::flat(4_000.0, 7).with_pad(pad);
    let transform = terrain.home_transform(pad);
    (terrain, transform)
}

/// Add a landscape to the scene (one per scene).
pub fn create(world: &mut World, editor: &mut Editor) {
    let t = now(world);
    if let Some(existing) = find(world) {
        editor.select(Some(existing));
        editor.error(t, "The scene already has a landscape.");
        return;
    }
    let (terrain, transform) = new_terrain();
    let terrain = match serde_json::to_value(&terrain) {
        Ok(v) => v,
        Err(e) => {
            editor.error(t, format!("Could not create the landscape: {e}"));
            return;
        }
    };
    editor::spawn_with(
        world,
        editor,
        "Landscape".into(),
        vec![("PlanetTerrain", terrain), ("Sculpt", json!({}))],
        transform,
    );
    editor.tool = editor::Tool::Sculpt;
}

/// The terrain edit for one dab.
pub fn edit_for(dab: &Dab, materials: &SurfaceMaterials) -> TerrainEdit {
    let at = DVec3::from(dab.at);
    let radius = dab.radius.max(0.1) as f64;
    // Raise and lower: a sphere whose cap pokes `amount` out of (or into) the ground under the
    // brush. Its size is tied to the brush, not to the amount, so dabs stacked on a growing
    // mound never reach further than one and a half brush radii from where they land.
    let h = (dab.amount as f64).max(0.005);
    let sphere = (radius * SPHERE_PER_RADIUS).max(h);
    let (center, shape, op, surface) = match &dab.op {
        SculptOp::Raise => (
            at - DVec3::Y * (sphere - h),
            EditShape::Sphere {
                radius: sphere as f32,
            },
            EditOp::Add,
            None,
        ),
        SculptOp::Lower => (
            at + DVec3::Y * (sphere - h),
            EditShape::Sphere {
                radius: sphere as f32,
            },
            EditOp::Remove,
            None,
        ),
        SculptOp::Restore => (
            at,
            EditShape::Sphere {
                radius: radius as f32,
            },
            EditOp::Smooth {
                strength: dab.amount.clamp(0.0, 1.0),
            },
            None,
        ),
        SculptOp::Paint(name) => (
            at,
            EditShape::Sphere {
                radius: radius as f32,
            },
            EditOp::Paint,
            materials.id(name),
        ),
    };
    TerrainEdit {
        center,
        shape,
        op,
        surface,
        source: None,
    }
}

/// Send new dabs to the terrain; after an undo removed some, rebuild it from the rest
/// (Update).
pub fn sync_sculpt(
    mut commands: Commands,
    mut q: Query<(
        Entity,
        &Sculpt,
        Option<&PlanetTerrain>,
        Option<&mut SculptApplied>,
    )>,
    materials: Res<SurfaceMaterials>,
    mut edits: MessageWriter<TerrainEdit>,
) {
    for (entity, sculpt, terrain, applied) in &mut q {
        let Some(mut applied) = applied else {
            commands.entity(entity).insert(SculptApplied::default());
            continue;
        };
        if let Some(stash) = applied.rebuilding.take() {
            // The old terrain is gone (one frame without it): bring it back fresh.
            commands.entity(entity).insert(stash);
            applied.sent = 0;
            continue;
        }
        let Some(terrain) = terrain else {
            continue;
        };
        if sculpt.dabs.len() < applied.sent {
            applied.rebuilding = Some(terrain.clone());
            commands.entity(entity).remove::<PlanetTerrain>();
            continue;
        }
        for dab in &sculpt.dabs[applied.sent..] {
            edits.write(edit_for(dab, &materials));
        }
        applied.sent = sculpt.dabs.len();
    }
}

/// Where a ray from the camera meets the ground, if within `max` meters.
pub fn ground_hit(ground: &Ground, origin: DVec3, dir: Vec3, max: f64) -> Option<DVec3> {
    let dir = dir.as_dvec3();
    let above = |t: f64| {
        let p = origin + dir * t;
        ground.0.sample(p).map(|s| p.y - s.height)
    };
    let mut prev = 0.0;
    let mut t = 0.0;
    if above(0.0)? < 0.0 {
        return None;
    }
    while t < max {
        t += 0.25 + t * 0.02;
        let h = above(t)?;
        if h < 0.0 {
            // Bisect between the last point above and this one below.
            let (mut lo, mut hi) = (prev, t);
            for _ in 0..24 {
                let mid = 0.5 * (lo + hi);
                if above(mid).unwrap_or(-1.0) < 0.0 {
                    hi = mid;
                } else {
                    lo = mid;
                }
            }
            return Some(origin + dir * hi);
        }
        prev = t;
    }
    None
}

/// The brush in the scene view: preview circle under the cursor, dabs while the left button
/// is held (Shift lowers instead of raising).
pub fn brush(
    ui: &egui::Ui,
    world: &mut World,
    editor: &mut Editor,
    view: &ViewInfo,
    response: &egui::Response,
) {
    let Some(landscape) = find(world) else {
        return;
    };
    let Some(pos) = response.hover_pos().or(response.interact_pointer_pos()) else {
        return;
    };
    let (origin, dir) = view.ray(pos);
    let hit = ground_hit(world.resource::<Ground>(), origin, dir, 5_000.0);
    let Some(hit) = hit else {
        return;
    };
    let shift = ui.input(|i| i.modifiers.shift);
    let op = match (&editor.brush.op, shift) {
        (SculptOp::Raise, true) => SculptOp::Lower,
        (SculptOp::Lower, true) => SculptOp::Raise,
        (SculptOp::Paint(_), _) => SculptOp::Paint(PAINTS[editor.brush.paint].into()),
        (op, _) => op.clone(),
    };
    let color = match op {
        SculptOp::Raise => [0.45, 0.85, 0.45, 0.9],
        SculptOp::Lower => [0.95, 0.5, 0.35, 0.9],
        SculptOp::Restore => [0.5, 0.75, 1.0, 0.9],
        SculptOp::Paint(_) => [1.0, 0.85, 0.35, 0.9],
    };
    {
        let mut debug = world.resource_mut::<engine::debug::DebugDraw>();
        let lift = DVec3::Y * 0.05;
        debug
            .circle(hit + lift, Vec3::Y, editor.brush.radius, color)
            .on_top();
        debug
            .circle(
                hit + lift,
                Vec3::Y,
                editor.brush.radius * 0.5,
                [color[0], color[1], color[2], 0.35],
            )
            .on_top();
    }
    let held = response.is_pointer_button_down_on() && ui.input(|i| i.pointer.primary_down());
    if !held {
        editor.brush.since = f32::INFINITY;
        return;
    }
    editor.brush.since += ui.input(|i| i.stable_dt);
    if editor.brush.since < DAB_SECS {
        return;
    }
    editor.brush.since = 0.0;
    let amount = match op {
        SculptOp::Raise | SculptOp::Lower => editor.brush.radius * 0.04 * editor.brush.strength,
        _ => editor.brush.strength,
    };
    dab(
        world,
        editor,
        landscape,
        Dab {
            at: hit.to_array(),
            radius: editor.brush.radius,
            amount,
            op,
        },
    );
}

/// Add one dab as an undoable edit (a whole stroke merges into one step).
pub fn dab(world: &mut World, editor: &mut Editor, landscape: Entity, dab: Dab) {
    let Some(sculpt) = world.get::<Sculpt>(landscape) else {
        return;
    };
    let mut after = sculpt.clone();
    after.dabs.push(dab);
    let before = serde_json::to_value(sculpt).ok();
    let after = serde_json::to_value(&after).ok();
    editor::apply(
        world,
        editor,
        Command::Component {
            entity: landscape,
            name: "Sculpt".into(),
            before,
            after,
        },
    );
}

/// The brush panel, floating in the scene view while sculpting.
pub fn panel(ui: &mut egui::Ui, world: &mut World, editor: &mut Editor) {
    let Some(landscape) = find(world) else {
        ui.label(theme_dim("This scene has no landscape yet."));
        if ui.button("🌄 Create landscape").clicked() {
            create(world, editor);
        }
        return;
    };
    ui.horizontal(|ui| {
        let ops = [
            (SculptOp::Raise, "Raise"),
            (SculptOp::Lower, "Lower"),
            (SculptOp::Restore, "Restore"),
            (SculptOp::Paint(String::new()), "Paint"),
        ];
        for (op, label) in ops {
            let on = std::mem::discriminant(&op) == std::mem::discriminant(&editor.brush.op);
            if ui.selectable_label(on, label).clicked() {
                editor.brush.op = op;
            }
        }
    });
    egui::Grid::new("brush").num_columns(2).show(ui, |ui| {
        ui.label("Radius");
        ui.add(
            egui::Slider::new(&mut editor.brush.radius, 0.5..=60.0)
                .logarithmic(true)
                .suffix(" m"),
        );
        ui.end_row();
        ui.label("Strength");
        ui.add(egui::Slider::new(&mut editor.brush.strength, 0.05..=1.0));
        ui.end_row();
        if matches!(editor.brush.op, SculptOp::Paint(_)) {
            ui.label("Material");
            egui::ComboBox::from_id_salt("paint")
                .selected_text(PAINTS[editor.brush.paint])
                .show_ui(ui, |ui| {
                    for (i, name) in PAINTS.iter().enumerate() {
                        ui.selectable_value(&mut editor.brush.paint, i, *name);
                    }
                });
            ui.end_row();
        }
    });
    let dabs = world.get::<Sculpt>(landscape).map_or(0, |s| s.dabs.len());
    ui.label(theme_dim(format!(
        "{dabs} dabs · drag to sculpt · Shift lowers · Ctrl+Z undoes a stroke"
    )));
}

fn theme_dim(text: impl Into<String>) -> egui::RichText {
    crate::theme::dim(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_raise_dab_lifts_its_center_by_its_amount() {
        let materials = SurfaceMaterials::default();
        let dab = Dab {
            at: [3.0, 2.0, -1.0],
            radius: 5.0,
            amount: 0.4,
            op: SculptOp::Raise,
        };
        let edit = edit_for(&dab, &materials);
        let EditShape::Sphere { radius } = edit.shape else {
            panic!("sphere");
        };
        let top = edit.center.y + radius as f64;
        assert!((top - 2.4).abs() < 1e-4, "{top}");
        // Even on a mound 10 m up, the sphere stays within 1.5 brush radii.
        assert!((radius as f64) <= 7.5 + 1e-6);
        assert_eq!(edit.op, EditOp::Add);
    }

    #[test]
    fn paint_names_the_surface() {
        let materials = SurfaceMaterials::default();
        let dab = Dab {
            at: [0.0; 3],
            radius: 2.0,
            amount: 1.0,
            op: SculptOp::Paint("sand".into()),
        };
        let edit = edit_for(&dab, &materials);
        let id = edit.surface.expect("sand is a surface");
        assert_eq!(materials.get(id).name, "sand");
    }

    #[test]
    fn the_ground_hit_is_on_the_ground() {
        let ground = Ground::default();
        let hit = ground_hit(
            &ground,
            DVec3::new(0.0, 10.0, 0.0),
            Vec3::new(0.0, -1.0, -1.0).normalize(),
            100.0,
        )
        .unwrap();
        assert!(hit.y.abs() < 1e-4 && (hit.z + 10.0).abs() < 1e-3, "{hit}");
    }
}
