//! Moving the selection onto other things (Move tool). Hold V: the selection's corner nearest
//! the pointer is marked; drag it onto a corner of anything else and it lands there exactly
//! (Unity's and Unreal's vertex snapping). Hold Ctrl+Shift and drag: the selection slides over
//! whatever is under the pointer, resting its bottom on the surface (Unity's surface snapping;
//! Unreal's Surface Snapping, with Rotate to Surface Normal and Surface Offset). Each drag is
//! one undo step.

use std::collections::HashSet;

use engine::prelude::*;
use engine::render::{Mesh, ResolvedMesh};
use glam::{DAffine3, DQuat};

use crate::editor::{self, Editor, Tool};
use crate::scene_ops::EditorOnly;
use crate::undo::Command;
use crate::view::ViewInfo;

/// How near the pointer a corner has to be to snap to it, points.
const REACH: f32 = 24.0;
/// A bounding box corner wins over a mesh vertex this much nearer the pointer, points: rounded
/// and bevelled edges have many vertices near a corner, none of them on it.
const BOX_PREFERENCE: f32 = 10.0;

/// A corner on screen: where it is, where it shows, and whether it is a bounding box corner.
type Corner = (DVec3, egui::Pos2, bool);

/// The eight corners of a local box.
fn box_points(min: Vec3, max: Vec3) -> [Vec3; 8] {
    std::array::from_fn(|i| {
        Vec3::new(
            if i & 1 == 0 { min.x } else { max.x },
            if i & 2 == 0 { min.y } else { max.y },
            if i & 4 == 0 { min.z } else { max.z },
        )
    })
}

/// Which snap a drag does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Vertex,
    Surface,
}

/// A selected entity being moved: its transform before the drag and its world placement.
struct Moving {
    entity: Entity,
    before: Transform,
    parent_inv: DAffine3,
    scale: DVec3,
    rotation: DQuat,
    translation: DVec3,
}

/// A snap drag in progress.
pub struct Drag {
    kind: Kind,
    /// The point of the selection that lands on the target (a corner, or the primary's pivot).
    grab: DVec3,
    /// How far the selection's bottom is below the primary's pivot: along its own up axis
    /// (for tilting onto a surface) and straight down.
    below: f64,
    below_y: f64,
    moving: Vec<Moving>,
    /// Where it last landed, for the overlay.
    target: Option<DVec3>,
}

/// Which snap the held keys ask for, if any.
pub fn held(ui: &egui::Ui, editor: &Editor) -> Option<Kind> {
    if editor.tool != Tool::Move || editor.selection.is_empty() {
        return None;
    }
    if editor.snap_drag.is_some() {
        return editor.snap_drag.as_ref().map(|d| d.kind);
    }
    if ui.ctx().egui_wants_keyboard_input() {
        return None;
    }
    ui.input(|i| {
        if i.modifiers.ctrl && i.modifiers.shift {
            Some(Kind::Surface)
        } else if i.key_down(egui::Key::V) && !i.modifiers.ctrl {
            Some(Kind::Vertex)
        } else {
            None
        }
    })
}

/// The selected entities and everything under them.
fn moving_set(world: &World, editor: &Editor) -> HashSet<Entity> {
    let mut out = HashSet::new();
    let mut stack: Vec<Entity> = editor.selection.clone();
    while let Some(e) = stack.pop() {
        if out.insert(e)
            && let Some(children) = world.get::<Children>(e)
        {
            stack.extend(children.iter());
        }
    }
    out
}

/// Every visible mesh's world corners, for meshes `keep` names, whose bounds come near `pos`.
fn corners_near(
    world: &mut World,
    view: &ViewInfo,
    pos: egui::Pos2,
    keep: &dyn Fn(Entity) -> bool,
) -> Vec<Corner> {
    let mut q = world.query_filtered::<(Entity, &GlobalTransform, &ResolvedMesh, &MeshRenderer), Without<EditorOnly>>();
    let Some(meshes) = world.get_resource::<Assets<Mesh>>() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (entity, global, resolved, renderer) in q.iter(world) {
        if !renderer.visible || !keep(entity) {
            continue;
        }
        let Some(mesh) = meshes.get(resolved.mesh) else {
            continue;
        };
        // Skip meshes whose bounds are nowhere near the pointer on screen.
        let aabb = mesh.aabb();
        let mut area = egui::Rect::NOTHING;
        let mut behind = false;
        let mut box_corners = Vec::new();
        for corner in box_points(aabb.min, aabb.max) {
            let at = global.transform_point(corner.as_dvec3());
            match view.project(at) {
                Some(p) => {
                    area.extend_with(p);
                    box_corners.push((at, p, true));
                }
                None => behind = true,
            }
        }
        if !behind && !area.expand(REACH).contains(pos) {
            continue;
        }
        out.extend(box_corners);
        let mut seen = HashSet::new();
        for v in &mesh.vertices {
            // Split vertices (one per face for flat shading) share a corner.
            if !seen.insert(v.position.map(f32::to_bits)) {
                continue;
            }
            let at = global.transform_point(Vec3::from(v.position).as_dvec3());
            if let Some(p) = view.project(at) {
                out.push((at, p, false));
            }
        }
    }
    out
}

/// The world corners of `entity`'s own mesh's bounding box.
pub fn box_corners(world: &World, entity: Entity) -> Vec<DVec3> {
    let (Some(global), Some(resolved), Some(meshes)) = (
        world.get::<GlobalTransform>(entity),
        world.get::<ResolvedMesh>(entity),
        world.get_resource::<Assets<Mesh>>(),
    ) else {
        return Vec::new();
    };
    meshes.get(resolved.mesh).map_or(Vec::new(), |mesh| {
        let aabb = mesh.aabb();
        box_points(aabb.min, aabb.max)
            .iter()
            .map(|c| global.transform_point(c.as_dvec3()))
            .collect()
    })
}

/// The corner nearest `pos` on screen among `corners`, within `reach` points.
fn nearest(corners: &[Corner], pos: egui::Pos2, reach: f32) -> Option<DVec3> {
    corners
        .iter()
        .map(|(at, p, boxed)| (*at, p.distance(pos), *boxed))
        .filter(|(_, d, _)| *d <= reach)
        .min_by(|a, b| {
            let score = |(_, d, boxed): &(DVec3, f32, bool)| {
                if *boxed { d - BOX_PREFERENCE } else { *d }
            };
            score(a).total_cmp(&score(b))
        })
        .map(|(at, _, _)| at)
}

/// The selection's corner nearest the pointer (any distance), which a vertex drag grabs.
pub fn grab_corner(
    world: &mut World,
    editor: &Editor,
    view: &ViewInfo,
    pos: egui::Pos2,
) -> Option<DVec3> {
    let mine = moving_set(world, editor);
    let corners = corners_near(world, view, pos, &|e| mine.contains(&e));
    nearest(&corners, pos, f32::INFINITY)
}

/// A corner of something other than the selection near the pointer.
fn target_corner(
    world: &mut World,
    editor: &Editor,
    view: &ViewInfo,
    pos: egui::Pos2,
) -> Option<DVec3> {
    let mine = moving_set(world, editor);
    let corners = corners_near(world, view, pos, &|e| !mine.contains(&e));
    nearest(&corners, pos, REACH)
}

/// The corner of anything nearest `pos` within `reach` points (the Measure tool's ends).
pub fn corner_near(
    world: &mut World,
    view: &ViewInfo,
    pos: egui::Pos2,
    reach: f32,
) -> Option<DVec3> {
    let corners = corners_near(world, view, pos, &|_| true);
    nearest(&corners, pos, reach)
}

/// Start a snap drag of the selection, grabbing it at `grab` (vertex) or its pivot (surface).
pub fn begin(world: &mut World, editor: &mut Editor, kind: Kind, grab: Option<DVec3>) {
    let selected: HashSet<Entity> = editor.selection.iter().copied().collect();
    let mut moving = Vec::new();
    for &e in &editor.selection {
        // A child of something selected moves with it.
        let mut up = world.get::<ChildOf>(e).map(|p| p.parent());
        let mut nested = false;
        while let Some(p) = up {
            nested |= selected.contains(&p);
            up = world.get::<ChildOf>(p).map(|p| p.parent());
        }
        if nested || world.get::<EditorOnly>(e).is_some() {
            continue;
        }
        let (Some(before), Some(global)) =
            (world.get::<Transform>(e), world.get::<GlobalTransform>(e))
        else {
            continue;
        };
        let parent_inv = world
            .get::<ChildOf>(e)
            .and_then(|p| world.get::<GlobalTransform>(p.parent()))
            .map(|g| g.inverse())
            .unwrap_or(DAffine3::IDENTITY);
        let (scale, rotation, translation) = global.affine().to_scale_rotation_translation();
        moving.push(Moving {
            entity: e,
            before: *before,
            parent_inv,
            scale,
            rotation,
            translation,
        });
    }
    let Some(first) = moving.first() else {
        return;
    };
    let grab = grab.unwrap_or(first.translation);
    // The bottom: the lowest corner of the primary's meshes along its up axis.
    let up = first.rotation * DVec3::Y;
    let pivot = first.translation;
    let primary = first.entity;
    let mut below: f64 = 0.0;
    let mut below_y: f64 = 0.0;
    let mut stack = vec![primary];
    let meshes = world.get_resource::<Assets<Mesh>>();
    while let Some(e) = stack.pop() {
        if let Some(children) = world.get::<Children>(e) {
            stack.extend(children.iter());
        }
        let (Some(global), Some(resolved), Some(meshes)) = (
            world.get::<GlobalTransform>(e),
            world.get::<ResolvedMesh>(e),
            meshes,
        ) else {
            continue;
        };
        let Some(mesh) = meshes.get(resolved.mesh) else {
            continue;
        };
        for v in &mesh.vertices {
            let at = global.transform_point(Vec3::from(v.position).as_dvec3());
            below = below.min((at - pivot).dot(up));
            below_y = below_y.min(at.y - pivot.y);
        }
    }
    editor.snap_drag = Some(Drag {
        kind,
        grab,
        below: -below,
        below_y: -below_y,
        moving,
        target: None,
    });
}

/// Move the dragged selection so its grab point lands on `target`; for a surface snap, `normal`
/// is the surface's and the selection rests its bottom on it.
pub fn land(world: &mut World, editor: &mut Editor, target: DVec3, normal: Option<Vec3>) {
    let (align, offset) = (editor.surface_align, editor.surface_offset as f64);
    let Some(drag) = editor.snap_drag.as_mut() else {
        return;
    };
    drag.target = Some(target);
    let first_rotation = drag.moving[0].rotation;
    let (turn, goal) = match normal {
        Some(n) => {
            let n = n.as_dvec3();
            let turn = if align {
                DQuat::from_rotation_arc(first_rotation * DVec3::Y, n)
            } else {
                DQuat::IDENTITY
            };
            let lift = if align {
                n * (drag.below + offset)
            } else {
                DVec3::Y * (drag.below_y + offset)
            };
            (turn, target + lift)
        }
        None => (DQuat::IDENTITY, target),
    };
    let pivot = drag.moving[0].translation;
    // The grab point turns with the selection about the primary's pivot.
    let delta = goal - (pivot + turn * (drag.grab - pivot));
    for m in &drag.moving {
        let at = pivot + turn * (m.translation - pivot) + delta;
        let affine = DAffine3::from_scale_rotation_translation(m.scale, turn * m.rotation, at);
        let local = editor::transform_from_affine(m.parent_inv * affine);
        if let Some(mut t) = world.get_mut::<Transform>(m.entity) {
            *t = local;
        }
    }
}

/// Finish the drag: one undo step, or nothing when nothing moved.
pub fn end(world: &mut World, editor: &mut Editor) {
    let Some(drag) = editor.snap_drag.take() else {
        return;
    };
    let mut commands = Vec::new();
    for m in &drag.moving {
        let Some(after) = world.get::<Transform>(m.entity).copied() else {
            continue;
        };
        if after == m.before {
            continue;
        }
        let (Ok(before), Ok(after)) = (serde_json::to_value(m.before), serde_json::to_value(after))
        else {
            continue;
        };
        commands.push(Command::Component {
            entity: m.entity,
            name: "Transform".into(),
            before: Some(before),
            after: Some(after),
        });
    }
    if commands.is_empty() {
        return;
    }
    let label = match drag.kind {
        Kind::Vertex => "Snap to corner",
        Kind::Surface => "Snap to surface",
    };
    let t = editor::now(world);
    editor
        .history
        .record(world, Command::Group(label.into(), commands), t);
}

/// The snap drags in the scene view, while V or Ctrl+Shift is held with the Move tool. True when
/// it took the pointer (the gizmo and box select stay out of it).
pub fn interact(
    ui: &egui::Ui,
    world: &mut World,
    editor: &mut Editor,
    view: &ViewInfo,
    response: &egui::Response,
) -> bool {
    let Some(kind) = held(ui, editor) else {
        end(world, editor);
        return false;
    };
    let Some(pos) = ui.input(|i| i.pointer.latest_pos()) else {
        return true;
    };
    let down = ui.input(|i| i.pointer.primary_down());
    if editor.snap_drag.is_none() {
        let grab = match kind {
            Kind::Vertex => grab_corner(world, editor, view, pos),
            Kind::Surface => None,
        };
        if let Some(grab) = grab {
            mark(ui, view, grab, theme_grab(), 6.0);
        }
        if down && response.is_pointer_button_down_on() {
            begin(world, editor, kind, grab);
        }
        return true;
    }
    if !down {
        end(world, editor);
        return true;
    }
    let (o, d) = view.ray(pos);
    match kind {
        Kind::Vertex => {
            let target = target_corner(world, editor, view, pos).or_else(|| {
                let mine = moving_set(world, editor);
                crate::picking::pick_surface(world, o, d, 10_000.0, &|e| mine.contains(&e))
            });
            if let Some(target) = target {
                land(world, editor, target, None);
            }
        }
        Kind::Surface => {
            let mine = moving_set(world, editor);
            if let Some((hit, normal)) =
                crate::picking::surface_and_normal(world, o, d, &|e| mine.contains(&e))
            {
                land(world, editor, hit, Some(normal));
            }
        }
    }
    if let Some(drag) = &editor.snap_drag
        && let Some(target) = drag.target
    {
        mark(ui, view, target, theme_target(), 7.0);
    }
    true
}

fn theme_grab() -> egui::Color32 {
    egui::Color32::from_rgb(0xff, 0xd8, 0x40)
}

fn theme_target() -> egui::Color32 {
    egui::Color32::from_rgb(0x5d, 0xe0, 0x7a)
}

/// A square marker on a corner.
fn mark(ui: &egui::Ui, view: &ViewInfo, at: DVec3, color: egui::Color32, half: f32) {
    let Some(p) = view.project(at) else {
        return;
    };
    let painter = ui.painter();
    let rect = egui::Rect::from_center_size(p, egui::vec2(half * 2.0, half * 2.0));
    painter.rect_filled(rect.shrink(half * 0.45), 1.0, color);
    painter.rect_stroke(
        rect,
        1.0,
        egui::Stroke::new(1.5, color),
        egui::StrokeKind::Outside,
    );
}

/// A one-line hint at the bottom of the scene view while the Move tool has a selection.
pub fn hint(ui: &egui::Ui, editor: &Editor, rect: egui::Rect) {
    if editor.tool != Tool::Move || editor.selection.is_empty() || editor.game_view {
        return;
    }
    let text = match editor.snap_drag.as_ref().map(|d| d.kind) {
        Some(Kind::Vertex) => "Snapping to corners: release to place",
        Some(Kind::Surface) => "Snapping to surfaces: release to place",
        None => "Hold V and drag a corner onto another · Ctrl+Shift drag onto surfaces",
    };
    ui.painter().text(
        rect.center_bottom() + egui::vec2(0.0, -10.0),
        egui::Align2::CENTER_BOTTOM,
        text,
        egui::FontId::proportional(12.5),
        egui::Color32::from_white_alpha(170),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearest_corner_within_reach() {
        let corners = [
            (DVec3::X, egui::pos2(100.0, 100.0), false),
            (DVec3::Y, egui::pos2(110.0, 100.0), false),
            (DVec3::NEG_X, egui::pos2(120.0, 100.0), true),
            (DVec3::Z, egui::pos2(300.0, 300.0), false),
        ];
        let at = |x: f32, y: f32, reach: f32| nearest(&corners, egui::pos2(x, y), reach);
        assert_eq!(at(104.0, 100.0, 24.0), Some(DVec3::X));
        // A box corner a little further off wins over a vertex.
        assert_eq!(at(112.0, 100.0, 24.0), Some(DVec3::NEG_X));
        // Nothing within reach; any distance finds the nearest.
        assert_eq!(at(250.0, 250.0, 24.0), None);
        assert_eq!(at(290.0, 290.0, f32::INFINITY), Some(DVec3::Z));
    }
}
