//! The Grips tab: how a character's hands hold what they hold, measured on the hand people
//! see, drawn over the scene view and tuned live.
//!
//! Pick anyone holding something (the Animation paddock's fighters, or any character with
//! `HeldItems`). The tab measures the chosen hand every frame as `engine_anim`'s grip check
//! does: the palm's gap to the handle, how far each finger is off or into it, whether the thumb
//! closes on the far side and the handle's size against the hand, each marked against the
//! limits of the grip research (engine-design/anim-grip-research.md). The scene view shows the
//! handle, the palm and every finger segment coloured by its gap, as Unreal's socket preview
//! and Meta's hand-grab pose editor show a grip on the hand that will hold it.
//!
//! The item's seat (slid along its handle, rolled about it) and each finger's extra curl are
//! the item's grip preset (`engine_anim::grip::GripPresets`): changing them re-seats every
//! hand holding that item at once, and Save writes them to `assets/anim/grips.json`, which
//! the engine builds in. Owned by the Animation thread (crates/anim).

use std::path::PathBuf;

use egui::{Color32, RichText};
use engine::debug::DebugDraw;
use engine::prelude::*;
use engine_anim::Animator;
use engine_anim::grip::{GRIPS_FILE, GripGeometry, GripPresets, HeldItems};
use engine_anim::retarget::Retarget;

use crate::editor::Editor;
use crate::scene_ops;
use crate::theme;

pub struct GripsTabPlugin;

impl Plugin for GripsTabPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<GripsTab>()
            .add_systems(Startup, load_presets);
    }
}

/// The Grips tab's state.
#[derive(Resource)]
pub struct GripsTab {
    /// The character shown.
    pub target: Option<Entity>,
    /// Which of its hands (0 left, 1 right).
    pub side: usize,
    /// Draw the grip over the scene view.
    pub overlay: bool,
    /// The presets as last loaded or saved, to tell unsaved changes.
    pub saved: Option<GripPresets>,
    /// The last save's outcome.
    pub status: String,
}

impl Default for GripsTab {
    fn default() -> Self {
        Self {
            target: None,
            side: 1,
            overlay: true,
            saved: None,
            status: String::new(),
        }
    }
}

/// Finger names, thumb first, as the presets' trims list them.
const FINGERS: [&str; 5] = ["Thumb", "Index", "Middle", "Ring", "Little"];

/// Limits (warn, fail), meters, from the grip research's validation table.
const PALM_LIMITS: (f32, f32) = (0.005, 0.010);
const SINK_LIMITS: (f32, f32) = (0.002, 0.005);
const GAP_LIMITS: (f32, f32) = (0.004, 0.008);
/// A comfortable handle's diameter over the hand's length (about 0.2 is best).
const SIZE_RANGE: (f32, f32) = (0.15, 0.25);

/// Overlay colours.
const HANDLE_COLOR: [f32; 4] = [0.35, 0.85, 1.0, 0.9];
const PALM_COLOR: [f32; 4] = [1.0, 0.85, 0.3, 1.0];
const TOUCH_COLOR: [f32; 4] = [0.4, 1.0, 0.45, 1.0];
const GAP_COLOR: [f32; 4] = [1.0, 0.8, 0.2, 1.0];
const SINK_COLOR: [f32; 4] = [1.0, 0.3, 0.3, 1.0];

/// Where the presets file is: the asset folder's `anim/grips.json`.
fn presets_path(world: &World) -> PathBuf {
    world
        .get_resource::<engine::assets::AssetServer>()
        .map(|s| s.root().to_path_buf())
        .unwrap_or_default()
        .join(GRIPS_FILE)
}

/// Read the presets file over the built-in grips, so a saved change holds without a rebuild.
fn load_presets(world: &mut World) {
    let path = presets_path(world);
    let mut status = String::new();
    let presets = match std::fs::read_to_string(&path) {
        Ok(text) => match GripPresets::parse(&text) {
            Ok(p) => p,
            Err(e) => {
                status = format!(
                    "{} doesn't read ({e}): using the built-in grips",
                    path.display()
                );
                GripPresets::builtin()
            }
        },
        Err(_) => GripPresets::builtin(),
    };
    let mut tab = world.resource_mut::<GripsTab>();
    tab.saved = Some(presets.clone());
    tab.status = status;
    world.insert_resource(presets);
}

/// Everyone holding something: (entity, label).
fn holders(world: &mut World) -> Vec<(Entity, String)> {
    let mut q = world.query::<(Entity, &HeldItems)>();
    let mut out: Vec<(Entity, String)> = q
        .iter(world)
        .filter(|(_, h)| h.hands.iter().any(Option::is_some))
        .map(|(e, h)| {
            let items: Vec<&str> = h
                .presets
                .iter()
                .zip(&h.hands)
                .filter(|(_, hand)| hand.is_some())
                .map(|(p, _)| if p.is_empty() { "item" } else { p.as_str() })
                .collect();
            (e, items.join(" + "))
        })
        .collect();
    out.sort_by_key(|(e, _)| *e);
    for (e, items) in &mut out {
        *items = format!("{} · {items}", scene_ops::label(world, *e));
    }
    out
}

/// The holder the selection is, or is inside.
fn selected_holder(world: &World, editor: &Editor) -> Option<Entity> {
    let mut e = editor.primary()?;
    loop {
        if world.get::<HeldItems>(e).is_some() {
            return Some(e);
        }
        e = world.get::<ChildOf>(e)?.parent();
    }
}

/// Measure `target`'s hand `side` this frame.
fn measure(world: &World, target: Entity, side: usize) -> Option<GripGeometry> {
    let animator = world.get::<Animator>(target)?;
    let retarget = world.get::<Retarget>(target)?;
    let handle = world.get::<HeldItems>(target)?.hands[side]?;
    retarget.retargeter.grip_geometry(
        &animator.model,
        &animator.skeleton,
        &retarget.model,
        side,
        &handle,
    )
}

/// The Grips tab.
pub fn panel(ui: &mut egui::Ui, world: &mut World, editor: &mut Editor) {
    let mut tab = world.remove_resource::<GripsTab>().unwrap_or_default();
    if let Some(e) = selected_holder(world, editor) {
        tab.target = Some(e);
    }
    if tab.target.is_some_and(|e| world.get_entity(e).is_err()) {
        tab.target = None;
    }
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| body(ui, world, editor, &mut tab));
    world.insert_resource(tab);
}

fn body(ui: &mut egui::Ui, world: &mut World, editor: &mut Editor, tab: &mut GripsTab) {
    let list = holders(world);
    if list.is_empty() {
        ui.add_space(16.0);
        ui.vertical_centered(|ui| {
            ui.label(theme::dim(
                "Nobody here holds anything. Place the Animation paddock (its fighters hold a \
                 sword, a shield, a great sword and a bow) or give a character HeldItems.",
            ))
        });
        return;
    }
    let target = *tab.target.get_or_insert(list[0].0);
    ui.horizontal(|ui| {
        let shown = list
            .iter()
            .find(|(e, _)| *e == target)
            .map_or("Pick someone".to_string(), |(_, l)| l.clone());
        egui::ComboBox::from_id_salt("grips holder")
            .selected_text(shown)
            .width(260.0)
            .show_ui(ui, |ui| {
                for (e, label) in &list {
                    ui.selectable_value(&mut tab.target, Some(*e), label);
                }
            });
        ui.selectable_value(&mut tab.side, 0, "Left hand");
        ui.selectable_value(&mut tab.side, 1, "Right hand");
        ui.checkbox(&mut tab.overlay, "Overlay");
    });
    let Some(held) = world.get::<HeldItems>(target).cloned() else {
        return;
    };
    if held.hands[tab.side].is_none()
        && let Some(other) = (0..2).find(|&s| held.hands[s].is_some())
    {
        tab.side = other;
    }
    let side = tab.side;
    let Some(handle) = held.hands[side] else {
        return;
    };
    let name = held.presets[side].clone();
    ui.add_space(6.0);

    let geometry = measure(world, target, side);
    heading(ui, "📊 Fit");
    match &geometry {
        None => {
            ui.label(theme::dim(
                "This character's visible hand isn't loaded yet (its model is fetched with the \
                 animation art), so there is nothing to measure.",
            ));
        }
        Some(g) => fit_table(ui, g),
    }
    if tab.overlay
        && !editor.game_view
        && let Some(g) = &geometry
    {
        draw_overlay(world, target, g);
    }

    ui.add_space(6.0);
    heading(ui, "🔧 Seat");
    if name.is_empty() {
        ui.label(theme::dim(
            "This hand's grip has no preset, so it can be measured but not tuned here.",
        ));
        return;
    }
    let mut presets = world.resource::<GripPresets>().clone();
    let preset = presets.0.entry(name.clone()).or_default();
    let before = *preset;
    egui::Grid::new("grips seat")
        .num_columns(2)
        .spacing([12.0, 4.0])
        .show(ui, |ui| {
            ui.label("Preset");
            ui.label(RichText::new(&name).strong());
            ui.end_row();
            ui.label("Handle");
            ui.label(theme::dim(format!(
                "{:.0} mm thick, {:.0} mm long (the item's mesh)",
                handle.radius * 2000.0,
                (handle.extent[1] - handle.extent[0]) * 1000.0
            )));
            ui.end_row();
            ui.label("Slide")
                .on_hover_text("Move the item along its handle; the hand stays where it is");
            let mut mm = preset.seat.slide * 1000.0;
            if ui
                .add(egui::Slider::new(&mut mm, -60.0..=60.0).suffix(" mm"))
                .changed()
            {
                preset.seat.slide = mm / 1000.0;
            }
            ui.end_row();
            ui.label("Roll")
                .on_hover_text("Turn the item about its handle (a blade's edge in the hand)");
            ui.add(egui::Slider::new(&mut preset.seat.roll, -180.0..=180.0).suffix("°"));
            ui.end_row();
        });
    ui.add_space(6.0);
    heading(ui, "✋ Fingers");
    egui::Grid::new("grips fingers")
        .num_columns(3)
        .striped(true)
        .spacing([12.0, 4.0])
        .show(ui, |ui| {
            ui.label(theme::dim("Finger"));
            ui.label(theme::dim("Closest to handle"));
            ui.label(theme::dim("Extra curl"));
            ui.end_row();
            for (f, finger) in FINGERS.iter().enumerate() {
                ui.label(*finger);
                match geometry.as_ref().and_then(|g| g.fingers.get(f)) {
                    Some(segments) => {
                        let gap = segments.iter().map(|s| s.2).fold(f32::MAX, f32::min);
                        let limits = if gap < 0.0 { SINK_LIMITS } else { GAP_LIMITS };
                        // The thumb wraps round the far side; its gap isn't judged.
                        let color = if f == 0 {
                            theme::TEXT
                        } else {
                            grade(gap.abs(), limits)
                        };
                        ui.label(RichText::new(mm(gap)).color(color));
                    }
                    None => {
                        ui.label(theme::dim("-"));
                    }
                }
                ui.add(egui::Slider::new(&mut preset.trims[f], -30.0..=30.0).suffix("°"));
                ui.end_row();
            }
        });
    let changed = *preset != before;
    if changed {
        world.insert_resource(presets);
    }

    ui.add_space(8.0);
    let current = world.resource::<GripPresets>().clone();
    let dirty = tab.saved.as_ref() != Some(&current);
    ui.horizontal(|ui| {
        let path = presets_path(world);
        if ui
            .add_enabled(dirty, egui::Button::new("💾 Save grips"))
            .on_hover_text(path.display().to_string())
            .clicked()
        {
            tab.status = match std::fs::write(&path, current.to_json()) {
                Ok(()) => {
                    tab.saved = Some(current.clone());
                    format!("Saved {}", path.display())
                }
                Err(e) => format!("Couldn't save {}: {e}", path.display()),
            };
        }
        if ui
            .add_enabled(dirty, egui::Button::new("↺ Revert"))
            .clicked()
            && let Some(saved) = tab.saved.clone()
        {
            world.insert_resource(saved);
            tab.status = "Reverted to the saved grips".into();
        }
        if dirty {
            ui.label(RichText::new("Unsaved changes").color(theme::WARN));
        } else if !tab.status.is_empty() {
            ui.label(theme::dim(&tab.status));
        }
    });
    ui.add_space(6.0);
    ui.label(theme::dim(
        "Green: within the limits. Yellow: noticeable. Red: wrong. In the scene view the handle \
         is blue, the palm yellow, and each finger segment green where it touches the handle, \
         yellow where it stands off it and red where it sinks in.",
    ));
}

fn fit_table(ui: &mut egui::Ui, g: &GripGeometry) {
    let r = &g.report;
    egui::Grid::new("grips fit")
        .num_columns(3)
        .striped(true)
        .spacing([12.0, 4.0])
        .show(ui, |ui| {
            row(
                ui,
                "Palm to handle",
                mm(r.palm),
                grade(r.palm.abs(), PALM_LIMITS),
                "The palm's skin under the knuckles to the handle's surface",
            );
            row(
                ui,
                "Deepest finger",
                mm(-r.deepest),
                grade(r.deepest, SINK_LIMITS),
                "How far the worst finger sinks into the handle",
            );
            row(
                ui,
                "Widest finger gap",
                mm(r.widest),
                grade(r.widest, GAP_LIMITS),
                "The finger standing furthest off the handle",
            );
            let thumb = if r.thumb_opposed {
                ("Far side", theme::PLAY)
            } else {
                ("Same side", Color32::from_rgb(235, 80, 80))
            };
            row(
                ui,
                "Thumb",
                thumb.0.into(),
                thumb.1,
                "A power grip's thumb closes on the far side of the handle from the palm",
            );
            let size = r.size;
            let size_color = if (SIZE_RANGE.0..=SIZE_RANGE.1).contains(&size) {
                theme::PLAY
            } else {
                theme::WARN
            };
            row(
                ui,
                "Handle size",
                format!("{:.0}% of the hand", size * 100.0),
                size_color,
                "Handle diameter over hand length: 15 to 25% is comfortable, about 20% best",
            );
        });
}

fn row(ui: &mut egui::Ui, what: &str, value: String, color: Color32, help: &str) {
    ui.label(what).on_hover_text(help);
    ui.label(RichText::new(value).color(color).strong());
    let mark = if color == theme::PLAY { "○" } else { "◼" };
    ui.label(RichText::new(mark).color(color));
    ui.end_row();
}

/// A length in millimetres, signed (a gap is positive, sinking in negative).
fn mm(m: f32) -> String {
    // Adding 0 turns a -0 into 0, so a touch reads +0.0.
    format!("{:+.1} mm", m * 1000.0 + 0.0)
}

/// Green within the warn limit, yellow to the fail limit, red past it.
fn grade(value: f32, (warn, fail): (f32, f32)) -> Color32 {
    if value <= warn {
        theme::PLAY
    } else if value <= fail {
        theme::WARN
    } else {
        Color32::from_rgb(235, 80, 80)
    }
}

fn heading(ui: &mut egui::Ui, text: &str) {
    ui.add_space(4.0);
    ui.label(RichText::new(text).strong().size(14.0).color(theme::TEXT));
    ui.add_space(2.0);
}

/// The handle, the palm and the finger segments over the scene view.
fn draw_overlay(world: &mut World, target: Entity, g: &GripGeometry) {
    let Some(gt) = world.get::<GlobalTransform>(target).copied() else {
        return;
    };
    let (origin, rot, scale) = (gt.translation(), gt.rotation(), gt.scale());
    let at = |p: Vec3| origin + (rot * (p * scale)).as_dvec3();
    let mut debug = world.resource_mut::<DebugDraw>();
    debug
        .cylinder(at(g.handle[0]), at(g.handle[1]), g.radius, HANDLE_COLOR)
        .on_top();
    debug.sphere(at(g.palm), 0.004, PALM_COLOR).on_top();
    debug
        .arrow(at(g.palm), at(g.palm + g.palm_dir * 0.03), PALM_COLOR)
        .on_top();
    for (f, finger) in g.fingers.iter().enumerate() {
        for &(a, b, gap) in finger {
            let color = if gap < -SINK_LIMITS.0 {
                SINK_COLOR
            } else if gap > GAP_LIMITS.0 && f > 0 {
                GAP_COLOR
            } else {
                TOUCH_COLOR
            };
            debug.line(at(a), at(b), color).on_top();
            debug.sphere(at(b), 0.002, color).on_top();
        }
    }
}

/// The shot script: the Animation paddock's fighters, each grip measured and drawn, then a
/// tuned grip (`editor --headless --size 1920x1080 --shots <dir> --script grips`).
pub fn shots(dir: String) -> crate::shots::ShotScript {
    use crate::editor::{self, Create, Tab};
    use crate::shots::Step;
    use egui_dock::{DockState, NodeIndex};

    let setup = |world: &mut World, editor: &mut Editor| {
        editor.icons = false;
        let mut dock = DockState::new(vec![Tab::Viewport]);
        let surface = dock.main_surface_mut();
        let _ = surface.split_right(NodeIndex::root(), 0.62, vec![Tab::Grips]);
        world.resource_mut::<crate::ui::EditorDock>().0 = dock;
        editor::create(world, editor, Create::Demo("anim"), DVec3::ZERO);
    };
    // Show `item`'s holder's hand `side`, the camera close on it from the side.
    let show = |item: &'static str, side: usize| {
        move |world: &mut World, editor: &mut Editor| {
            let mut q = world.query::<(Entity, &HeldItems)>();
            let Some(e) = q
                .iter(world)
                .find(|(_, h)| h.presets[side] == item)
                .map(|(e, _)| e)
            else {
                return;
            };
            {
                let mut tab = world.resource_mut::<GripsTab>();
                tab.target = Some(e);
                tab.side = side;
            }
            editor.select(None);
            let (Some(a), Some(gt)) = (world.get::<Animator>(e), world.get::<GlobalTransform>(e))
            else {
                return;
            };
            let Some(hand) = a.skeleton.index_of(["hand_l", "hand_r"][side]) else {
                return;
            };
            let p = gt.translation() + (gt.rotation() * (a.model[hand].t * gt.scale())).as_dvec3();
            // From the hand's outer side, a little in front and above.
            let out = gt.rotation() * if side == 0 { Vec3::NEG_X } else { Vec3::X };
            let front = gt.rotation() * Vec3::NEG_Z;
            let eye = p + (out * 0.42 + front * 0.3 + Vec3::Y * 0.12).as_dvec3();
            if let Some(mut t) = world.get_mut::<Transform>(editor.camera) {
                *t = Transform::from_translation(eye).looking_at(p, Vec3::Y);
                let fwd = t.forward();
                if let Some(mut cam) = world.get_mut::<crate::camera::EditorCamera>(editor.camera) {
                    *cam = crate::camera::EditorCamera::looking(fwd);
                }
            }
        }
    };
    let tune = |world: &mut World, _: &mut Editor| {
        let mut presets = world.resource::<GripPresets>().clone();
        let sword = presets.0.entry("sword".into()).or_default();
        sword.trims = [0.0, -25.0, -25.0, -10.0, 0.0];
        sword.seat.roll = 35.0;
        world.insert_resource(presets);
    };
    let steps = vec![
        Step::Do(Box::new(setup)),
        Step::Tab(Tab::Grips),
        Step::Wait(60),
        Step::Do(Box::new(|world: &mut World, ed: &mut Editor| {
            editor::play(world, ed)
        })),
        Step::Wait(90),
        Step::Do(Box::new(|world: &mut World, ed: &mut Editor| {
            editor::pause(world, ed)
        })),
        Step::Do(Box::new(show("sword", 1))),
        Step::Wait(6),
        Step::Shot("grips_01_sword"),
        Step::Do(Box::new(show("shield", 0))),
        Step::Wait(6),
        Step::Shot("grips_02_shield"),
        Step::Do(Box::new(show("greatsword", 0))),
        Step::Wait(6),
        Step::Shot("grips_03_greatsword_off_hand"),
        Step::Do(Box::new(show("bow", 0))),
        Step::Wait(6),
        Step::Shot("grips_04_bow"),
        Step::Do(Box::new(show("hammer", 1))),
        Step::Wait(6),
        Step::Shot("grips_05_hammer"),
        Step::Do(Box::new(show("sword", 1))),
        Step::Do(Box::new(tune)),
        Step::Wait(6),
        Step::Shot("grips_06_sword_tuned"),
    ];
    crate::shots::ShotScript::new(dir, steps)
}
