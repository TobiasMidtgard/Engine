//! The scene view tab: the 3D scene shows through it, with the transform gizmo, click
//! selection, icons for lights, cameras and demos, the tool bar and a stats overlay on top.

use engine::debug::DebugDraw;
use engine::prelude::*;
use transform_gizmo_egui::{
    EnumSet, GizmoConfig, GizmoExt, GizmoMode, GizmoOrientation, GizmoVisuals,
};

use crate::editor::{self, Editor, Layout, Mode, Pane, Tool};
use crate::models::Dragged;
use crate::picking;
use crate::scene_ops::{EditorOnly, InScene};
use crate::theme;
use crate::undo::Command;
use crate::view::{ViewInfo, mint_rows};

/// Screen radius of an icon, points.
const ICON_RADIUS: f32 = 11.0;

/// Gap between scene views, points.
const PANE_GAP: f32 = 2.0;

/// The scene tab: one, two or four scene views (Unreal's viewport layouts, 3ds Max's and
/// Blender's quad view), each with its own camera. The view under the pointer is the active
/// one: the camera keys, the tool bar and the gizmo work there.
pub fn panes(ui: &mut egui::Ui, world: &mut World, editor: &mut Editor) {
    let rect = ui.max_rect();
    editor.tab_rect = Some(rect);
    ensure_panes(world, editor);
    let shown = pane_rects(editor, rect);
    // A layout of one view shows the active one; make sure it is.
    if shown.len() == 1 && shown[0].0 != editor.active_pane {
        crate::camera_preview::stop_pilot(world, editor);
        editor.activate_pane(shown[0].0);
    }
    // The pointer picks the active view, but never in the middle of something (a drag, a fly,
    // a box, a pilot), so a drag that leaves its view stays with it.
    let busy = ui.input(|i| i.pointer.any_down())
        || ui.ctx().dragged_id().is_some()
        || editor.gizmo.is_focused()
        || editor.box_start.is_some()
        || crate::camera_preview::piloting(world).is_some()
        || world
            .get::<crate::camera::EditorCamera>(editor.camera)
            .is_some_and(|c| c.is_dragging());
    if !busy
        && let Some(pos) = ui.input(|i| i.pointer.hover_pos())
        && let Some(&(i, _)) = shown.iter().find(|(_, r)| r.contains(pos))
    {
        editor.activate_pane(i);
    }
    sync_cameras(world, editor, &shown);

    for &(i, r) in &shown {
        if i != editor.active_pane {
            editor.swap_pane(i);
        }
        editor.pane = i;
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(r)
                .id_salt(("scene view", i)),
        );
        scene_view(&mut child, world, editor);
        if i != editor.active_pane {
            editor.swap_pane(i);
        }
    }
    editor.pane = editor.active_pane;
    if let Some(i) = editor.maximize_request.take() {
        crate::camera_preview::stop_pilot(world, editor);
        editor.activate_pane(i);
        toggle_maximized(world, editor);
    }

    if shown.len() > 1 {
        let painter = ui.painter_at(rect);
        // The gaps between views, and the active one outlined (as Unreal's yellow frame).
        let mut gaps = vec![rect];
        for &(_, r) in &shown {
            gaps = gaps.into_iter().flat_map(|g| subtract(g, r)).collect();
        }
        for g in gaps {
            painter.rect_filled(g, 0, theme::BORDER);
        }
        if let Some(&(_, r)) = shown.iter().find(|(i, _)| *i == editor.active_pane) {
            painter.rect_stroke(
                r,
                0,
                egui::Stroke::new(1.5, theme::ACCENT),
                egui::StrokeKind::Inside,
            );
        }
    }
}

/// `a` without `b`: up to four rectangles.
fn subtract(a: egui::Rect, b: egui::Rect) -> Vec<egui::Rect> {
    let b = a.intersect(b);
    if !b.is_positive() {
        return vec![a];
    }
    [
        egui::Rect::from_min_max(a.min, egui::pos2(a.max.x, b.min.y)),
        egui::Rect::from_min_max(egui::pos2(a.min.x, b.max.y), a.max),
        egui::Rect::from_min_max(egui::pos2(a.min.x, b.min.y), egui::pos2(b.min.x, b.max.y)),
        egui::Rect::from_min_max(egui::pos2(b.max.x, b.min.y), egui::pos2(a.max.x, b.max.y)),
    ]
    .into_iter()
    .filter(|r| r.is_positive())
    .collect()
}

/// The axis views of the quad layout: top, front and right, orthographic, looking at what the
/// perspective view looks at (3ds Max's and Blender's quad view).
const AXIS_PANES: [Vec3; 3] = [Vec3::NEG_Y, Vec3::NEG_Z, Vec3::NEG_X];

/// The other views' cameras, made the first time a layout needs them.
fn ensure_panes(world: &mut World, editor: &mut Editor) {
    if editor.layout == Layout::One || editor.panes.len() > AXIS_PANES.len() {
        return;
    }
    let (pivot, distance) = match (
        world.get::<Transform>(editor.camera),
        world.get::<crate::camera::EditorCamera>(editor.camera),
    ) {
        (Some(t), Some(c)) => (
            t.translation + t.forward().as_dvec3() * c.pivot_distance,
            c.pivot_distance.max(12.0),
        ),
        _ => (DVec3::ZERO, 20.0),
    };
    for forward in AXIS_PANES {
        let camera = crate::camera::pane_camera(world, forward, pivot, distance, true);
        editor.panes.push(Pane::new(camera));
    }
}

/// The views the layout shows and where (pane 0 is the perspective view, then top, front and
/// right).
pub fn pane_rects(editor: &Editor, rect: egui::Rect) -> Vec<(usize, egui::Rect)> {
    let half = PANE_GAP * 0.5;
    let mid = rect.center();
    let left = egui::Rect::from_min_max(rect.min, egui::pos2(mid.x - half, rect.max.y));
    let right = egui::Rect::from_min_max(egui::pos2(mid.x + half, rect.min.y), rect.max);
    let top = |r: egui::Rect| egui::Rect::from_min_max(r.min, egui::pos2(r.max.x, mid.y - half));
    let bottom = |r: egui::Rect| egui::Rect::from_min_max(egui::pos2(r.min.x, mid.y + half), r.max);
    let one = vec![(editor.active_pane, rect)];
    match (editor.layout, editor.maximized) {
        (Layout::One, _) => one,
        (_, Some(i)) if i < editor.panes.len() => vec![(i, rect)],
        _ if editor.panes.len() <= AXIS_PANES.len() => one,
        (Layout::Two, _) => vec![(0, left), (1, right)],
        // Top and front above, right and perspective below, as 3ds Max lays them out.
        (Layout::Four, _) => vec![
            (1, top(left)),
            (2, top(right)),
            (3, bottom(left)),
            (0, bottom(right)),
        ],
    }
}

/// The camera of pane `i` (the active pane's lives in the editor).
fn pane_camera(editor: &Editor, i: usize) -> Entity {
    if i == editor.active_pane {
        editor.camera
    } else {
        editor.panes[i].camera
    }
}

/// Shown views render, the rest don't. One shown view is the main view (the perspective one when
/// it is shown, at full quality); the others draw as extra views into their panes.
fn sync_cameras(world: &mut World, editor: &Editor, shown: &[(usize, egui::Rect)]) {
    let main = if shown.iter().any(|&(i, _)| i == 0) {
        0
    } else {
        shown[0].0
    };
    for i in 0..editor.panes.len() {
        let e = pane_camera(editor, i);
        let on = shown.iter().any(|&(j, _)| j == i);
        let Ok(mut entity) = world.get_entity_mut(e) else {
            continue;
        };
        if let Some(mut camera) = entity.get_mut::<Camera>()
            && camera.active != on
        {
            camera.active = on;
        }
        let extra = entity.contains::<ExtraView>();
        if on && i == main && extra {
            entity.remove::<ExtraView>();
        } else if on && i != main && !extra {
            entity.insert(
                ExtraView::window().with_quality(engine::world::Scalability::preset(
                    engine::world::QualityPreset::Medium,
                )),
            );
        }
    }
}

/// Switch the scene tab's layout.
pub fn set_layout(world: &mut World, editor: &mut Editor, layout: Layout) {
    // Flying a camera from one view must not carry over to another.
    crate::camera_preview::stop_pilot(world, editor);
    editor.layout = layout;
    editor.maximized = None;
    // Back to one view is back to the perspective view, not whichever axis view was active:
    // Blender does this so leaving quad view never loses the user's view.
    if layout == Layout::One {
        editor.activate_pane(0);
    }
}

/// Let the active view fill the scene tab, or put the layout back (Blender's Ctrl+Space).
pub fn toggle_maximized(world: &mut World, editor: &mut Editor) {
    if editor.layout == Layout::One {
        return;
    }
    crate::camera_preview::stop_pilot(world, editor);
    editor.maximized = match editor.maximized {
        Some(_) => None,
        None => Some(editor.active_pane),
    };
}

/// The layout menu in the tool bar.
fn layout_menu(ui: &mut egui::Ui, world: &mut World, editor: &mut Editor) {
    let label = match (editor.layout, editor.maximized) {
        (Layout::One, _) => "1 view ⏷",
        (_, Some(_)) => "Maximized ⏷",
        (Layout::Two, _) => "2 views ⏷",
        (Layout::Four, _) => "4 views ⏷",
    };
    ui.menu_button(label, |ui| {
        for (layout, name) in [
            (Layout::One, "One view"),
            (Layout::Two, "Two side by side"),
            (Layout::Four, "Four: top, front, right, perspective"),
        ] {
            let on = editor.layout == layout && editor.maximized.is_none();
            if ui.selectable_label(on, name).clicked() {
                set_layout(world, editor, layout);
                ui.close();
            }
        }
        ui.separator();
        let text = if editor.maximized.is_some() {
            "Restore the views"
        } else {
            "Maximize this view"
        };
        if ui
            .add_enabled(
                editor.layout != Layout::One,
                egui::Button::new(text).shortcut_text("Ctrl+Space"),
            )
            .clicked()
        {
            toggle_maximized(world, editor);
            ui.close();
        }
        ui.label(theme::dim("Ctrl+Alt+Q: four views or one"));
    })
    .response
    .on_hover_text("Scene view layout: one, two or four views, each with its own camera");
}

/// With several views, each names its view at its bottom left, with a button to maximize it.
fn pane_label(ui: &mut egui::Ui, editor: &mut Editor, pane: egui::Rect) {
    if editor.layout == Layout::One {
        return;
    }
    let Some(view) = editor.view else {
        return;
    };
    let ortho = view.ortho.is_some();
    let name = crate::camera::view_name(view.rotation * Vec3::NEG_Z, ortho);
    let i = editor.pane;
    let mut toggle = false;
    egui::Area::new(egui::Id::new(("pane label", i)))
        .fixed_pos(pane.left_bottom() + egui::vec2(8.0, -32.0))
        .order(egui::Order::Middle)
        .show(ui.ctx(), |ui| {
            egui::Frame::new()
                .fill(egui::Color32::from_rgba_unmultiplied(22, 23, 26, 200))
                .corner_radius(4)
                .inner_margin(egui::Margin::symmetric(6, 2))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let color = if i == editor.active_pane {
                            theme::ACCENT
                        } else {
                            theme::TEXT
                        };
                        ui.label(egui::RichText::new(name).color(color).strong());
                        let text = if editor.maximized.is_some() {
                            "Restore"
                        } else {
                            "Maximize"
                        };
                        toggle = ui.small_button(text).on_hover_text("Ctrl+Space").clicked();
                    });
                });
        });
    if toggle {
        // Applied once every view is drawn (this one's state is swapped in now).
        editor.maximize_request = Some(i);
    }
}

pub fn scene_view(ui: &mut egui::Ui, world: &mut World, editor: &mut Editor) {
    let rect = ui.max_rect();
    let response = ui.allocate_rect(rect, egui::Sense::click_and_drag());
    set_viewport(world, editor, rect, ui.ctx().pixels_per_point());
    let Some(view) = camera_view(world, editor, rect) else {
        return;
    };
    editor.view = Some(view);
    editor.viewport_hovered = response.contains_pointer();

    let painter = ui.painter_at(rect);
    // World-space helpers show in every view: draw them once, for the active one.
    let active = editor.pane == editor.active_pane;
    if !editor.game_view && active {
        draw_selection(world, editor);
    }
    if editor.grid && !editor.game_view && active {
        draw_grid(world, &view);
    }
    let icons = if editor.icons && !editor.game_view {
        draw_icons(&painter, world, editor, &view)
    } else {
        Vec::new()
    };

    let snapping = crate::snap::interact(ui, world, editor, &view, &response);
    let gizmo_used = !snapping && !editor.game_view && gizmo(ui, world, editor, &view);
    drop_target(ui, world, editor, &view, &response);

    let alt = ui.input(|i| i.modifiers.alt);
    if editor.tool == Tool::Sculpt {
        if !alt {
            crate::landscape::brush(ui, world, editor, &view, &response);
        }
        overlays(ui, world, editor, rect);
        return;
    }
    if editor.tool == Tool::Scatter {
        if !alt {
            crate::scatter::brush(ui, world, editor, &view, &response);
        }
        overlays(ui, world, editor, rect);
        return;
    }
    if editor.tool == Tool::Measure {
        if !alt {
            crate::measure::brush(ui, world, editor, &view, &response);
        }
        overlays(ui, world, editor, rect);
        return;
    }
    if response.clicked() && !gizmo_used && !snapping && !alt && !editor.gizmo.is_focused() {
        let additive = ui.input(|i| i.modifiers.ctrl || i.modifiers.shift);
        if let Some(pos) = response.interact_pointer_pos() {
            click_select(world, editor, &view, &icons, pos, additive);
        }
    }
    if response.double_clicked() && editor.primary().is_some() {
        editor::focus_selection(world, editor);
    }
    box_select(
        ui,
        world,
        editor,
        &view,
        &response,
        alt || gizmo_used || snapping,
    );
    // The snap hint runs along the bottom of a lone view; with several it would cover their
    // labels.
    if editor.layout == Layout::One || editor.maximized.is_some() {
        crate::snap::hint(ui, editor, rect);
    }

    overlays(ui, world, editor, rect);
}

/// Models and prefabs dragged from the Content panel land where the pointer meets a surface
/// (or 8 m out in the air).
fn drop_target(
    ui: &egui::Ui,
    world: &mut World,
    editor: &mut Editor,
    view: &ViewInfo,
    response: &egui::Response,
) {
    // A drag from another panel leaves `hovered` false, so ask the pointer directly.
    if !response.contains_pointer() {
        return;
    }
    let Some(pos) = ui.input(|i| i.pointer.hover_pos()) else {
        return;
    };
    let (o, d) = view.ray(pos);
    // Files from the desktop.
    if let Some(path) = &editor.file_hover {
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        ui.painter().text(
            pos + egui::vec2(16.0, -18.0),
            egui::Align2::LEFT_BOTTOM,
            format!("Drop to import {name}"),
            egui::FontId::proportional(13.0),
            theme::ACCENT,
        );
    }
    if !editor.file_drops.is_empty() {
        let at =
            picking::pick_surface(world, o, d, 200.0, &|_| false).unwrap_or(o + d.as_dvec3() * 8.0);
        let onto = crate::materials::target(world, o, d);
        crate::import::import_dropped(world, editor, Some(at), onto);
    }
    if let Some(dragged) = response.dnd_hover_payload::<Dragged>() {
        let label = match &*dragged {
            Dragged::Model(key) if crate::materials::is_material(key) => {
                let name = crate::materials::display_name(key);
                match crate::materials::target(world, o, d) {
                    Some(e) => format!(
                        "Drop to put {name} on {}",
                        crate::scene_ops::label(world, e)
                    ),
                    None => format!("{name} is a material: drop it on an object"),
                }
            }
            Dragged::Model(key) => {
                let name = crate::models::display_name(key);
                match crate::rigs::clip_target(world, key, o, d) {
                    Some(rig) => {
                        let on = world.get::<Name>(rig).map_or("it", |n| n.as_str());
                        format!("Drop to play {name} on {on}")
                    }
                    None if crate::rigs::is_clip_file(world, key) => {
                        format!("{name} is an animation: drop it on a character")
                    }
                    None => format!("Drop to place {name}"),
                }
            }
            Dragged::Prefab(path) => format!("Drop to place {}", crate::prefab::display_name(path)),
        };
        ui.painter().text(
            pos + egui::vec2(16.0, -18.0),
            egui::Align2::LEFT_BOTTOM,
            label,
            egui::FontId::proportional(13.0),
            theme::ACCENT,
        );
    }
    let Some(dragged) = response.dnd_release_payload::<Dragged>() else {
        return;
    };
    let at =
        picking::pick_surface(world, o, d, 200.0, &|_| false).unwrap_or(o + d.as_dvec3() * 8.0);
    match &*dragged {
        Dragged::Model(key) if crate::materials::is_material(key) => {
            if let Some(e) = crate::materials::target(world, o, d) {
                crate::materials::apply(world, editor, key, &[e]);
            }
        }
        Dragged::Model(key) => match crate::rigs::clip_target(world, key, o, d) {
            Some(rig) => crate::rigs::add_clip_file(world, editor, rig, key),
            None => {
                crate::models::place(world, editor, key, at);
            }
        },
        Dragged::Prefab(path) => {
            crate::prefab::place(world, editor, path, Some(at), None);
        }
    }
}

/// Draw the editor camera into the scene tab only (physical pixels).
fn set_viewport(world: &mut World, editor: &Editor, rect: egui::Rect, ppp: f32) {
    let min = (rect.min.to_vec2() * ppp).round();
    let size = (rect.size() * ppp).round();
    let viewport = Viewport::new(
        UVec2::new(min.x.max(0.0) as u32, min.y.max(0.0) as u32),
        UVec2::new(size.x.max(1.0) as u32, size.y.max(1.0) as u32),
    );
    if world.get::<Viewport>(editor.camera) != Some(&viewport) {
        world.entity_mut(editor.camera).insert(viewport);
    }
}

/// The editor camera's view, mapped to the scene tab.
fn camera_view(world: &World, editor: &Editor, screen: egui::Rect) -> Option<ViewInfo> {
    let t = world.get::<Transform>(editor.camera)?;
    let camera = world.get::<Camera>(editor.camera)?;
    let (fov_y, near, ortho) = match camera.projection {
        Projection::Perspective { fov_y_deg, near } => (fov_y_deg.to_radians(), near, None),
        Projection::Orthographic { height, .. } => (60f32.to_radians(), 0.05, Some(height)),
    };
    Some(ViewInfo {
        origin: t.translation,
        rotation: t.rotation,
        fov_y,
        near,
        ortho,
        screen,
    })
}

/// Selected entities as camera-relative gizmo targets, plus what's needed to write them back.
struct GizmoTarget {
    entity: Entity,
    before: Transform,
    parent_inv: glam::DAffine3,
}

/// Run the transform gizmo; true while it is being used.
fn gizmo(ui: &mut egui::Ui, world: &mut World, editor: &mut Editor, view: &ViewInfo) -> bool {
    let modes: EnumSet<GizmoMode> = match editor.tool {
        Tool::Select | Tool::Sculpt | Tool::Scatter | Tool::Measure => return false,
        Tool::Move => GizmoMode::all_translate(),
        Tool::Rotate => GizmoMode::all_rotate(),
        Tool::Scale => GizmoMode::all_scale(),
    };
    let mut targets = Vec::new();
    let mut gizmo_transforms = Vec::new();
    for &e in &editor.selection {
        if world.get::<EditorOnly>(e).is_some() {
            continue;
        }
        // A piloted camera moves with the view, not the gizmo.
        if crate::camera_preview::piloting(world) == Some(e) {
            continue;
        }
        // Locked things and folders hold still.
        if crate::hide::is_locked(world, &editor.hide, e) || crate::folder::is_folder(world, e) {
            continue;
        }
        let (Some(local), Some(global)) =
            (world.get::<Transform>(e), world.get::<GlobalTransform>(e))
        else {
            continue;
        };
        let parent_inv = world
            .get::<ChildOf>(e)
            .and_then(|p| world.get::<GlobalTransform>(p.parent()))
            .map(|g| g.inverse())
            .unwrap_or(glam::DAffine3::IDENTITY);
        let (scale, rotation, translation) = global.affine().to_scale_rotation_translation();
        gizmo_transforms.push(
            transform_gizmo_egui::math::Transform::from_scale_rotation_translation(
                to_mint3(scale),
                to_mint_quat(rotation),
                to_mint3(translation - view.origin),
            ),
        );
        targets.push(GizmoTarget {
            entity: e,
            before: *local,
            parent_inv,
        });
    }
    if targets.is_empty() {
        return false;
    }
    let ppp = ui.ctx().pixels_per_point();
    editor.gizmo.update_config(GizmoConfig {
        view_matrix: mint_rows(view.gizmo_view()),
        projection_matrix: mint_rows(view.gizmo_proj()),
        viewport: view.screen,
        modes,
        orientation: if editor.local_space {
            GizmoOrientation::Local
        } else {
            GizmoOrientation::Global
        },
        snapping: editor.snap,
        snap_angle: editor.snap_angle_deg.to_radians(),
        snap_distance: editor.snap_move,
        snap_scale: editor.snap_scale,
        visuals: GizmoVisuals {
            x_color: gizmo_color(theme::AXIS_COLORS[0]),
            y_color: gizmo_color(theme::AXIS_COLORS[1]),
            z_color: gizmo_color(theme::AXIS_COLORS[2]),
            s_color: gizmo_color(egui::Color32::WHITE),
            highlight_color: Some(gizmo_color(egui::Color32::from_rgb(0xff, 0xd8, 0x40))),
            inactive_alpha: 0.85,
            stroke_width: 3.5,
            gizmo_size: 90.0,
            ..Default::default()
        },
        pixels_per_point: ppp,
        ..Default::default()
    });
    let Some((_, results)) = editor.gizmo.interact(ui, &gizmo_transforms) else {
        editor.gizmo_dragging = false;
        return editor.gizmo.is_focused();
    };
    // Alt as a drag starts: move copies and leave the originals where they were (Unreal).
    if !editor.gizmo_dragging && ui.input(|i| i.modifiers.alt) {
        let copies = editor::duplicate_offset(world, editor, DVec3::ZERO);
        for target in &mut targets {
            if let Some(&(_, copy)) = copies.iter().find(|c| c.0 == target.entity) {
                target.entity = copy;
            }
        }
    }
    editor.gizmo_dragging = true;
    let mut commands = Vec::new();
    for (target, new) in targets.iter().zip(results) {
        let scale = from_mint3(new.scale);
        let rotation = from_mint_quat(new.rotation);
        let translation = from_mint3(new.translation) + view.origin;
        let world_affine =
            glam::DAffine3::from_scale_rotation_translation(scale, rotation, translation);
        let local = editor::transform_from_affine(target.parent_inv * world_affine);
        let (Ok(before), Ok(after)) = (
            serde_json::to_value(target.before),
            serde_json::to_value(local),
        ) else {
            continue;
        };
        commands.push(Command::Component {
            entity: target.entity,
            name: "Transform".into(),
            before: Some(before),
            after: Some(after),
        });
    }
    let command = if commands.len() == 1 {
        commands.pop().expect("one command")
    } else {
        let label = match editor.tool {
            Tool::Rotate => "Move: rotate",
            Tool::Scale => "Move: scale",
            _ => "Move",
        };
        Command::Group(format!("{label} {} entities", commands.len()), commands)
    };
    editor::apply(world, editor, command);
    true
}

fn gizmo_color(c: egui::Color32) -> transform_gizmo_egui::Color32 {
    transform_gizmo_egui::Color32::from_rgb(c.r(), c.g(), c.b())
}

fn to_mint3(v: DVec3) -> transform_gizmo_egui::mint::Vector3<f64> {
    transform_gizmo_egui::mint::Vector3 {
        x: v.x,
        y: v.y,
        z: v.z,
    }
}

fn from_mint3(v: transform_gizmo_egui::mint::Vector3<f64>) -> DVec3 {
    DVec3::new(v.x, v.y, v.z)
}

fn to_mint_quat(q: glam::DQuat) -> transform_gizmo_egui::mint::Quaternion<f64> {
    transform_gizmo_egui::mint::Quaternion {
        v: to_mint3(DVec3::new(q.x, q.y, q.z)),
        s: q.w,
    }
}

fn from_mint_quat(q: transform_gizmo_egui::mint::Quaternion<f64>) -> glam::DQuat {
    glam::DQuat::from_xyzw(q.v.x, q.v.y, q.v.z, q.s).normalize()
}

/// Select what is under the cursor: icons first, then meshes.
fn click_select(
    world: &mut World,
    editor: &mut Editor,
    view: &ViewInfo,
    icons: &[(Entity, egui::Pos2)],
    pos: egui::Pos2,
    additive: bool,
) {
    let icon_hit = icons
        .iter()
        .filter(|(e, _)| crate::hide::pickable(world, &editor.hide, *e))
        .filter(|(_, p)| p.distance(pos) < ICON_RADIUS + 3.0)
        .min_by(|a, b| a.1.distance(pos).total_cmp(&b.1.distance(pos)))
        .map(|(e, _)| *e);
    let hit = icon_hit.or_else(|| {
        let (o, d) = view.ray(pos);
        // Hidden and locked things can't be clicked.
        let hide = editor.hide.clone();
        let blocked: std::collections::HashSet<Entity> =
            if hide.is_empty() && hide.locked.is_empty() {
                Default::default()
            } else {
                let mut q = world.query_filtered::<Entity, With<GlobalTransform>>();
                let all: Vec<Entity> = q.iter(world).collect();
                all.into_iter()
                    .filter(|&e| !crate::hide::pickable(world, &hide, e))
                    .collect()
            };
        picking::pick_mesh_except(world, o, d, &|e| blocked.contains(&e))
            .map(|(e, _)| editor::selectable(world, e))
            .filter(|&e| crate::hide::pickable(world, &hide, e))
    });
    match (hit, additive) {
        (Some(e), true) => editor.toggle(e),
        (Some(e), false) => editor.select(Some(e)),
        (None, false) => editor.select(None),
        (None, true) => {}
    }
}

/// Drag with the left button on empty space to select everything whose center falls inside the
/// box (Ctrl or Shift adds to the selection). Children of something already in the box are left
/// out, so a group selects as one.
fn box_select(
    ui: &egui::Ui,
    world: &mut World,
    editor: &mut Editor,
    view: &ViewInfo,
    response: &egui::Response,
    blocked: bool,
) {
    if response.drag_started_by(egui::PointerButton::Primary)
        && !blocked
        && !editor.gizmo.is_focused()
    {
        editor.box_start = response.interact_pointer_pos();
    }
    let Some(start) = editor.box_start else {
        return;
    };
    let Some(end) = ui.input(|i| i.pointer.latest_pos()) else {
        return;
    };
    let area = egui::Rect::from_two_pos(start, end);
    let painter = ui.ctx().layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new("box select"),
    ));
    painter.rect(
        area,
        0,
        theme::ACCENT.gamma_multiply(0.12),
        egui::Stroke::new(1.0, theme::ACCENT),
        egui::StrokeKind::Inside,
    );
    if !response.drag_stopped() {
        return;
    }
    editor.box_start = None;
    if area.width() < 4.0 && area.height() < 4.0 {
        return;
    }
    let mut found = entities_in_box(world, view, area);
    found.retain(|&e| crate::hide::pickable(world, &editor.hide, e));
    let additive = ui.input(|i| i.modifiers.ctrl || i.modifiers.shift);
    if !additive {
        editor.selection.clear();
    }
    for e in found {
        if !editor.is_selected(e) {
            editor.selection.push(e);
        }
    }
}

/// Scene entities whose center projects inside `area`, topmost ones only.
pub fn entities_in_box(world: &mut World, view: &ViewInfo, area: egui::Rect) -> Vec<Entity> {
    let mut q = world.query_filtered::<Entity, (With<InScene>, With<GlobalTransform>)>();
    let candidates: Vec<Entity> = q.iter(world).collect();
    let inside: Vec<Entity> = candidates
        .into_iter()
        .filter(|&e| {
            editor::bounds(world, e)
                .and_then(|(center, _)| view.project(center))
                .is_some_and(|p| area.contains(p))
        })
        .collect();
    let mut found: Vec<Entity> = inside
        .iter()
        .copied()
        .filter(|&e| {
            !inside
                .iter()
                .any(|&other| other != e && crate::scene_ops::is_ancestor(world, other, e))
        })
        .collect();
    found.sort_by_key(|e| e.index());
    found
}

/// Helpers for the selection: colliders, a point light's range, a sun's direction. Meshes get their
/// silhouette outlined by [`crate::outline`].
fn draw_selection(world: &mut World, editor: &Editor) {
    for &e in &editor.selection {
        if editor.show_colliders {
            crate::collision::draw(world, e);
        }
        let Some(&global) = world.get::<GlobalTransform>(e) else {
            continue;
        };
        if let Some(&light) = world.get::<PointLight>(e) {
            let mut debug = world.resource_mut::<DebugDraw>();
            debug.sphere(global.translation(), light.range, [1.0, 0.85, 0.4, 0.35]);
        }
        if world.get::<DirectionalLight>(e).is_some() {
            let p = global.translation();
            let f = global.forward().as_dvec3();
            world
                .resource_mut::<DebugDraw>()
                .arrow(p, p + f * 3.0, [1.0, 0.9, 0.4, 1.0])
                .on_top();
        }
    }
}

/// A ground grid around the camera that fades out with distance (1 m cells, brighter every
/// 10 m; 10 m and 100 m when the camera is high), so it never draws a hard edge on the horizon.
fn draw_grid(world: &mut World, view: &ViewInfo) {
    let height = view.origin.y.abs();
    let cell = if height > 60.0 { 10.0 } else { 1.0 };
    let radius = cell * 30.0;
    let center = DVec3::new(
        (view.origin.x / cell).round() * cell,
        0.004,
        (view.origin.z / cell).round() * cell,
    );
    let eye = DVec3::new(view.origin.x, 0.0, view.origin.z);
    let fade = |p: DVec3| {
        let d = ((p - eye).length() / radius).min(1.0) as f32;
        (1.0 - d) * (1.0 - d)
    };
    let mut debug = world.resource_mut::<DebugDraw>();
    let n = 30i32;
    for i in -n..=n {
        for axis in 0..2 {
            let line = |t: f64| {
                let (u, v) = (i as f64 * cell, t * cell);
                if axis == 0 {
                    center + DVec3::new(u, 0.0, v)
                } else {
                    center + DVec3::new(v, 0.0, u)
                }
            };
            let base = if axis == 0 { center.x } else { center.z };
            let index = (base / cell).round() as i64 + i as i64;
            let alpha = if index % 10 == 0 { 0.34 } else { 0.14 };
            for j in -n..n {
                let (a, b) = (line(j as f64), line(j as f64 + 1.0));
                let k = fade((a + b) * 0.5);
                if k > 0.01 {
                    debug.line(a, b, [0.6, 0.62, 0.7, alpha * k]);
                }
            }
        }
    }
}

/// Icons for scene things without a mesh; returns their screen positions for clicking.
fn draw_icons(
    painter: &egui::Painter,
    world: &mut World,
    editor: &Editor,
    view: &ViewInfo,
) -> Vec<(Entity, egui::Pos2)> {
    let mut q = world.query_filtered::<(Entity, &GlobalTransform), With<InScene>>();
    let candidates: Vec<(Entity, DVec3)> =
        q.iter(world).map(|(e, g)| (e, g.translation())).collect();
    let mut out = Vec::new();
    for (e, at) in candidates {
        let Some(kind) = editor::has_icon(world, e) else {
            continue;
        };
        if crate::hide::is_hidden(world, &editor.hide, e) {
            continue;
        }
        let Some(pos) = view.project(at) else {
            continue;
        };
        if !painter.clip_rect().contains(pos) {
            continue;
        }
        let selected = editor.is_selected(e);
        let fill = if selected {
            theme::ACCENT
        } else {
            egui::Color32::from_rgba_unmultiplied(20, 22, 26, 210)
        };
        painter.circle(
            pos,
            ICON_RADIUS,
            fill,
            egui::Stroke::new(1.5, egui::Color32::from_white_alpha(200)),
        );
        let glyph = match kind {
            "sun" => "☀",
            "light" => "💡",
            "camera" => "🎥",
            "demo" => "💠",
            "model" => "🎲",
            _ => "○",
        };
        painter.text(
            pos,
            egui::Align2::CENTER_CENTER,
            glyph,
            egui::FontId::proportional(12.0),
            egui::Color32::WHITE,
        );
        if kind == "demo" || kind == "model" {
            painter.text(
                pos + egui::vec2(ICON_RADIUS + 5.0, 0.0),
                egui::Align2::LEFT_CENTER,
                crate::scene_ops::label(world, e),
                egui::FontId::proportional(12.0),
                egui::Color32::from_white_alpha(230),
            );
        }
        out.push((e, pos));
    }
    out
}

/// Tool bar (top left), stats (top right) and the play-mode frame.
fn overlays(ui: &mut egui::Ui, world: &mut World, editor: &mut Editor, pane: egui::Rect) {
    let ctx = ui.ctx().clone();
    pane_label(ui, editor, pane);
    axis_widget(&ctx, world, editor, pane);
    // With several views, each has its label and axis widget; the tool bar, stats and bars
    // belong to the whole tab (drawn once, with the active view).
    if editor.pane != editor.active_pane {
        return;
    }
    let rect = editor.tab_rect.unwrap_or(pane);
    egui::Area::new(egui::Id::new("viewport tools"))
        .fixed_pos(rect.min + egui::vec2(8.0, 8.0))
        .order(egui::Order::Middle)
        .show(&ctx, |ui| {
            egui::Frame::new()
                .fill(egui::Color32::from_rgba_unmultiplied(22, 23, 26, 225))
                .corner_radius(5)
                .inner_margin(egui::Margin::symmetric(5, 4))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 3.0;
                        for (tool, icon, tip) in [
                            (Tool::Select, "⬉", "Select (Q)"),
                            (Tool::Move, "✚", "Move (W)"),
                            (Tool::Rotate, "🔄", "Rotate (E)"),
                            (Tool::Scale, "⛶", "Scale (R)"),
                            (Tool::Sculpt, "🌄", "Sculpt the landscape (T)"),
                            (
                                Tool::Scatter,
                                "🌲",
                                "Paint rocks, trees and props onto surfaces (G)",
                            ),
                            (
                                Tool::Measure,
                                "📐",
                                "Measure: drag between two points; ends snap to corners (M)",
                            ),
                        ] {
                            let on = editor.tool == tool;
                            if ui
                                .add(
                                    egui::Button::new(egui::RichText::new(icon).size(15.0))
                                        .selected(on)
                                        .min_size(egui::vec2(26.0, 24.0)),
                                )
                                .on_hover_text(tip)
                                .clicked()
                            {
                                editor.tool = tool;
                            }
                        }
                        ui.separator();
                        let space = if editor.local_space { "Local" } else { "World" };
                        if ui
                            .add(
                                egui::Button::new(format!("🌐 {space}"))
                                    .min_size(egui::vec2(0.0, 24.0)),
                            )
                            .on_hover_text("Gizmo axes in world or local space")
                            .clicked()
                        {
                            editor.local_space = !editor.local_space;
                        }
                        ui.add(
                            egui::Button::new("📏 Snap")
                                .selected(editor.snap)
                                .min_size(egui::vec2(0.0, 24.0)),
                        )
                        .on_hover_text("Snap moves, rotations and scales")
                        .clicked()
                        .then(|| editor.snap = !editor.snap);
                        if editor.snap {
                            ui.add(
                                egui::DragValue::new(&mut editor.snap_move)
                                    .speed(0.05)
                                    .range(0.01..=100.0)
                                    .suffix(" m"),
                            );
                            ui.add(
                                egui::DragValue::new(&mut editor.snap_angle_deg)
                                    .speed(1.0)
                                    .range(1.0..=90.0)
                                    .suffix("°"),
                            );
                        }
                        if editor.tool == Tool::Move {
                            ui.toggle_value(&mut editor.surface_align, "Tilt to surface")
                                .on_hover_text(
                                    "Ctrl+Shift drags turn the selection's up to the surface",
                                );
                            ui.add(
                                egui::DragValue::new(&mut editor.surface_offset)
                                    .speed(0.01)
                                    .range(-10.0..=10.0)
                                    .suffix(" m"),
                            )
                            .on_hover_text("How far Ctrl+Shift drags hold it off the surface");
                        }
                        ui.separator();
                        let view = ui
                            .menu_button(format!("🎨 {}", crate::show::view_name(world)), |ui| {
                                crate::show::view_menu(ui, world)
                            })
                            .response
                            .on_hover_text(
                                "View mode: the lit scene or one of the renderer's buffers",
                            );
                        editor.view_button = Some(view.rect);
                        let show = ui
                            .menu_button(
                                if editor.game_view {
                                    "👁 Game view"
                                } else {
                                    "👁 Show"
                                },
                                |ui| crate::show::show_menu(ui, world, editor),
                            )
                            .response
                            .on_hover_text("Editor helpers and module debug drawing");
                        editor.show_button = Some(show.rect);
                        ui.toggle_value(&mut editor.grid, "⊞")
                            .on_hover_text("Ground grid");
                        ui.toggle_value(&mut editor.icons, "💡")
                            .on_hover_text("Icons for lights, cameras and demos");
                        layout_menu(ui, world, editor);
                        let speed = world
                            .get::<crate::camera::EditorCamera>(editor.camera)
                            .map(|c| c.speed)
                            .unwrap_or(0.0);
                        ui.label(theme::dim(format!("🎥 {speed:.1} m/s")))
                            .on_hover_text(
                                "Hold right mouse: look and fly with WASD, Q/E; wheel sets speed.\n\
                                 Middle drag: pan. Alt + left drag: orbit. F: frame selection.",
                            );
                    });
                });
        });
    if editor.tool == Tool::Sculpt {
        egui::Area::new(egui::Id::new("landscape brush"))
            .fixed_pos(rect.min + egui::vec2(8.0, 46.0))
            .order(egui::Order::Middle)
            .show(&ctx, |ui| {
                egui::Frame::new()
                    .fill(egui::Color32::from_rgba_unmultiplied(22, 23, 26, 235))
                    .corner_radius(5)
                    .inner_margin(egui::Margin::same(8))
                    .show(ui, |ui| {
                        ui.set_max_width(300.0);
                        ui.label(egui::RichText::new("🌄 Landscape").strong());
                        crate::landscape::panel(ui, world, editor);
                    });
            });
    }
    if editor.tool == Tool::Scatter {
        egui::Area::new(egui::Id::new("scatter brush"))
            .fixed_pos(rect.min + egui::vec2(8.0, 46.0))
            .order(egui::Order::Middle)
            .show(&ctx, |ui| {
                egui::Frame::new()
                    .fill(egui::Color32::from_rgba_unmultiplied(22, 23, 26, 235))
                    .corner_radius(5)
                    .inner_margin(egui::Margin::same(8))
                    .show(ui, |ui| {
                        ui.set_max_width(320.0);
                        ui.label(egui::RichText::new("🌲 Scatter").strong());
                        crate::scatter::panel(ui, world, editor);
                    });
            });
    }

    // Stats.
    let entities = world.entities().count_spawned();
    let mut scene_q = world.query_filtered::<(), With<InScene>>();
    let in_scene = scene_q.iter(world).count();
    let fps = 1.0 / editor.frame_secs.max(1e-4);
    let stats = format!(
        "{fps:.0} fps  ·  {:.1} ms\n{in_scene} scene / {entities} world entities",
        editor.frame_secs * 1000.0
    );
    let painter = ui.painter_at(rect);
    // Bottom right, clear of the toolbar (which grows with the view and show menus).
    let galley = painter.layout(
        stats,
        egui::FontId::monospace(11.5),
        egui::Color32::from_white_alpha(220),
        300.0,
    );
    let size = galley.size() + egui::vec2(16.0, 8.0);
    let bg = egui::Rect::from_min_size(rect.right_bottom() - size - egui::vec2(10.0, 10.0), size);
    painter.rect_filled(
        bg,
        4,
        egui::Color32::from_rgba_unmultiplied(22, 23, 26, 200),
    );
    painter.galley(bg.min + egui::vec2(8.0, 4.0), galley, egui::Color32::WHITE);

    crate::prefab_ui::editing_bar(&ctx, world, editor, rect);
    isolation_bar(&ctx, world, editor, rect);
    crate::camera_preview::ui(ui, world, editor, rect);

    // Play mode frame.
    if editor.mode != Mode::Edit {
        let (color, text) = if editor.mode == Mode::Play {
            (theme::PLAY, "▶  PLAYING  ·  Esc or Stop restores the scene")
        } else {
            (theme::WARN, "⏸  PAUSED")
        };
        painter.rect_stroke(
            rect.shrink(1.5),
            0,
            egui::Stroke::new(3.0, color),
            egui::StrokeKind::Inside,
        );
        let galley = painter.layout_no_wrap(
            text.into(),
            egui::FontId::proportional(13.0),
            egui::Color32::BLACK,
        );
        let pill = egui::Rect::from_center_size(
            egui::pos2(rect.center().x, rect.bottom() - 22.0),
            galley.size() + egui::vec2(24.0, 8.0),
        );
        painter.rect_filled(pill, 12, color);
        painter.galley(
            pill.min + egui::vec2(12.0, 4.0),
            galley,
            egui::Color32::BLACK,
        );
    }
}

/// Where the axis widget's middle is in the scene view `rect`.
pub fn axis_widget_center(rect: egui::Rect) -> egui::Pos2 {
    rect.right_top() + egui::vec2(-52.0, 98.0)
}

/// The axis widget under the stats (Unity's scene gizmo, Blender's navigation gizmo): the world
/// axes as the camera sees them. Clicking an axis looks along it from that side, orthographic;
/// clicking the middle switches perspective and orthographic. The view's name sits below.
fn axis_widget(ctx: &egui::Context, world: &World, editor: &mut Editor, rect: egui::Rect) {
    const RADIUS: f32 = 30.0;
    const KNOB: f32 = 8.5;
    let Some(view) = editor.view else {
        return;
    };
    let ortho = world
        .get::<crate::camera::EditorCamera>(editor.camera)
        .is_some_and(|c| c.ortho);
    let center = axis_widget_center(rect);
    let area =
        egui::Rect::from_center_size(center, egui::vec2(RADIUS * 2.0 + 24.0, RADIUS * 2.0 + 24.0));
    egui::Area::new(egui::Id::new(("axis widget", editor.pane)))
        .fixed_pos(area.min)
        .order(egui::Order::Middle)
        .show(ctx, |ui| {
            let response = ui.allocate_rect(
                egui::Rect::from_min_size(area.min, area.size() + egui::vec2(0.0, 16.0)),
                egui::Sense::click(),
            );
            let painter = ui.painter();
            let hover = response.hover_pos();
            painter.circle_filled(
                center,
                RADIUS + KNOB + 3.0,
                egui::Color32::from_rgba_unmultiplied(22, 23, 26, if hover.is_some() { 170 } else { 110 }),
            );
            // Each axis end: where it shows, how near it is, which way it points.
            let inverse = view.rotation.inverse();
            let mut ends: Vec<(egui::Pos2, f32, Vec3, usize, bool)> = Vec::new();
            for (i, axis) in [Vec3::X, Vec3::Y, Vec3::Z].into_iter().enumerate() {
                for positive in [true, false] {
                    let a = if positive { axis } else { -axis };
                    let v = inverse * a;
                    ends.push((center + egui::vec2(v.x, -v.y) * RADIUS, v.z, a, i, positive));
                }
            }
            // Far ends first, so the near ones draw over them.
            ends.sort_by(|a, b| a.1.total_cmp(&b.1));
            // An axis pointing at or away from the viewer sits on the middle, which is the
            // perspective switch then.
            let hovered = hover.and_then(|p| {
                ends.iter()
                    .rev()
                    .filter(|e| e.0.distance(center) > KNOB)
                    .find(|e| e.0.distance(p) <= KNOB + 2.0)
                    .map(|e| e.2)
            });
            for &(at, _, dir, i, positive) in &ends {
                let color = theme::AXIS_COLORS[i];
                let lit = hovered == Some(dir);
                if positive {
                    painter.line_segment([center, at], egui::Stroke::new(2.0, color));
                    painter.circle_filled(at, KNOB, if lit { egui::Color32::WHITE } else { color });
                    painter.text(
                        at,
                        egui::Align2::CENTER_CENTER,
                        ["X", "Y", "Z"][i],
                        egui::FontId::proportional(11.0),
                        egui::Color32::from_rgb(20, 20, 24),
                    );
                } else {
                    painter.circle(
                        at,
                        KNOB * 0.75,
                        color.gamma_multiply(if lit { 0.9 } else { 0.35 }),
                        egui::Stroke::new(1.5, color),
                    );
                }
            }
            let middle = hover.is_some_and(|p| p.distance(center) < KNOB) && hovered.is_none();
            painter.circle_filled(
                center,
                if middle { 5.0 } else { 3.5 },
                egui::Color32::from_white_alpha(if middle { 255 } else { 160 }),
            );
            let forward = view.rotation * Vec3::NEG_Z;
            let name = crate::camera::view_name(forward, ortho);
            let label = if ortho && name != "Orthographic" {
                format!("{name} · ortho")
            } else {
                name.to_string()
            };
            painter.text(
                center + egui::vec2(0.0, RADIUS + KNOB + 8.0),
                egui::Align2::CENTER_TOP,
                label,
                egui::FontId::proportional(11.5),
                egui::Color32::from_white_alpha(210),
            );
            if response.clicked() {
                // An axis end: look from that side toward the middle.
                editor.view_request = match hovered {
                    Some(dir) => Some(crate::camera::ViewRequest::Along(-dir)),
                    None if middle => Some(crate::camera::ViewRequest::Toggle),
                    None => None,
                };
            }
            response.on_hover_text(
                "Click an axis to look along it (orthographic), the middle for perspective.\n\
                 Numpad 7 / 1 / 3: top, front, right (Ctrl: the other side); 5 switches; 9 turns around.",
            );
        });
}

/// While isolating, a bar at the top of the view says so, with a way out (as Unity's).
fn isolation_bar(ctx: &egui::Context, world: &mut World, editor: &mut Editor, rect: egui::Rect) {
    let Some(isolated) = &editor.hide.isolated else {
        return;
    };
    let n = isolated.len();
    let mut exit = false;
    egui::Area::new(egui::Id::new("isolation bar"))
        .fixed_pos(egui::pos2(rect.center().x - 130.0, rect.min.y + 46.0))
        .order(egui::Order::Middle)
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(egui::Color32::from_rgba_unmultiplied(0x6a, 0x4a, 0x10, 235))
                .corner_radius(5)
                .inner_margin(egui::Margin::symmetric(10, 5))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(format!(
                                "Isolating {}",
                                if n == 1 {
                                    "1 object".to_string()
                                } else {
                                    format!("{n} objects")
                                }
                            ))
                            .color(egui::Color32::WHITE)
                            .strong(),
                        );
                        ui.label(
                            egui::RichText::new("Shift+H")
                                .color(egui::Color32::from_white_alpha(170)),
                        );
                        if ui.button("Exit").clicked() {
                            exit = true;
                        }
                    });
                });
        });
    if exit {
        crate::hide::toggle_isolation(world, editor);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::EditorCamera;

    /// Four views tile the tab with thin gaps, the active one keeps its state while the others
    /// are drawn, and only the perspective view renders as the main view.
    #[test]
    fn four_views_tile_the_tab_and_keep_their_cameras() {
        let mut world = World::new();
        let start = Transform::from_xyz(0.0, 5.0, 10.0).looking_at(DVec3::ZERO, Vec3::Y);
        let main = world
            .spawn((
                Camera::perspective(60.0),
                EditorCamera::looking(start.forward()),
                start,
            ))
            .id();
        let mut editor = Editor::new(main);
        let tab = egui::Rect::from_min_size(egui::pos2(0.0, 20.0), egui::vec2(800.0, 400.0));
        assert_eq!(pane_rects(&editor, tab), vec![(0, tab)]);

        editor.layout = Layout::Four;
        ensure_panes(&mut world, &mut editor);
        assert_eq!(editor.panes.len(), 4);
        let shown = pane_rects(&editor, tab);
        let order: Vec<usize> = shown.iter().map(|s| s.0).collect();
        assert_eq!(order, [1, 2, 3, 0]);
        let area: f32 = shown.iter().map(|s| s.1.area()).sum();
        assert!(tab.area() - area < 2.0 * PANE_GAP * 800.0, "{area}");
        let gaps = shown.iter().fold(vec![tab], |g, &(_, r)| {
            g.into_iter().flat_map(|g| subtract(g, r)).collect()
        });
        assert!(
            !gaps.is_empty()
                && gaps
                    .iter()
                    .all(|g| g.width() <= PANE_GAP + 0.01 || g.height() <= PANE_GAP + 0.01)
        );

        // The axis views look at what the perspective view looks at, orthographic.
        let top = editor.panes[1].camera;
        let t = world.get::<Transform>(top).unwrap();
        assert!(t.forward().dot(Vec3::NEG_Y) > 0.999);
        assert!(world.get::<EditorCamera>(top).unwrap().ortho);

        sync_cameras(&mut world, &editor, &shown);
        assert!(world.get::<ExtraView>(main).is_none());
        assert!(world.get::<ExtraView>(top).is_some());

        // Activating the top view swaps its camera in; the old one keeps its place.
        editor.activate_pane(1);
        assert_eq!(editor.camera, top);
        assert_eq!(pane_camera(&editor, 0), main);
        editor.activate_pane(0);
        assert_eq!(editor.camera, main);
        assert_eq!(pane_camera(&editor, 1), top);

        // Maximizing the front view makes it the only one and the main view.
        editor.activate_pane(2);
        toggle_maximized(&mut world, &mut editor);
        let shown = pane_rects(&editor, tab);
        assert_eq!(shown, vec![(2, tab)]);
        sync_cameras(&mut world, &editor, &shown);
        let front = pane_camera(&editor, 2);
        assert!(world.get::<ExtraView>(front).is_none());
        assert!(!world.get::<Camera>(main).unwrap().active);
        assert!(world.get::<Camera>(front).unwrap().active);

        // One view again is the perspective view (Blender's quad toggle).
        set_layout(&mut world, &mut editor, Layout::One);
        assert_eq!(editor.camera, main);
        assert_eq!(pane_rects(&editor, tab), vec![(0, tab)]);
    }
}
