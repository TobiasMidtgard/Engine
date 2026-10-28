//! The scene view tab: the 3D scene shows through it, with the transform gizmo, click
//! selection, icons for lights, cameras and demos, the tool bar and a stats overlay on top.

use engine::debug::DebugDraw;
use engine::prelude::*;
use transform_gizmo_egui::{
    EnumSet, GizmoConfig, GizmoExt, GizmoMode, GizmoOrientation, GizmoVisuals,
};

use crate::editor::{self, Editor, Mode, Tool};
use crate::models::Dragged;
use crate::picking;
use crate::scene_ops::{EditorOnly, InScene};
use crate::theme;
use crate::undo::Command;
use crate::view::{ViewInfo, mint_rows};

/// Screen radius of an icon, points.
const ICON_RADIUS: f32 = 11.0;

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
    draw_selection(world, editor);
    if editor.grid {
        draw_grid(world, &view);
    }
    let icons = if editor.icons {
        draw_icons(&painter, world, editor, &view)
    } else {
        Vec::new()
    };

    let snapping = crate::snap::interact(ui, world, editor, &view, &response);
    let gizmo_used = !snapping && gizmo(ui, world, editor, &view);
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
    crate::snap::hint(ui, editor, rect);

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
        Dragged::Prefab(path) => crate::prefab::place(world, editor, path, Some(at)),
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
    let (fov_y, near) = match camera.projection {
        Projection::Perspective { fov_y_deg, near } => (fov_y_deg.to_radians(), near),
        Projection::Orthographic { .. } => (60f32.to_radians(), 0.05),
    };
    Some(ViewInfo {
        origin: t.translation,
        rotation: t.rotation,
        fov_y,
        near,
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
        Tool::Select | Tool::Sculpt | Tool::Scatter => return false,
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
        return editor.gizmo.is_focused();
    };
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
        .filter(|(_, p)| p.distance(pos) < ICON_RADIUS + 3.0)
        .min_by(|a, b| a.1.distance(pos).total_cmp(&b.1.distance(pos)))
        .map(|(e, _)| *e);
    let hit = icon_hit.or_else(|| {
        let (o, d) = view.ray(pos);
        picking::pick_mesh(world, o, d).map(|(e, _)| editor::selectable(world, e))
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
    let found = entities_in_box(world, view, area);
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
        crate::collision::draw(world, e);
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
fn overlays(ui: &mut egui::Ui, world: &mut World, editor: &mut Editor, rect: egui::Rect) {
    let ctx = ui.ctx().clone();
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
                        ui.toggle_value(&mut editor.grid, "⊞")
                            .on_hover_text("Ground grid");
                        ui.toggle_value(&mut editor.icons, "💡")
                            .on_hover_text("Icons for lights, cameras and demos");
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
    let pos = rect.right_top() + egui::vec2(-10.0, 10.0);
    let galley = painter.layout(
        stats,
        egui::FontId::monospace(11.5),
        egui::Color32::from_white_alpha(220),
        300.0,
    );
    let bg = egui::Rect::from_min_size(
        pos - egui::vec2(galley.size().x + 8.0, -0.0),
        galley.size() + egui::vec2(16.0, 8.0),
    );
    let bg = bg.translate(egui::vec2(-8.0, 0.0));
    painter.rect_filled(
        bg,
        4,
        egui::Color32::from_rgba_unmultiplied(22, 23, 26, 200),
    );
    painter.galley(bg.min + egui::vec2(8.0, 4.0), galley, egui::Color32::WHITE);

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
