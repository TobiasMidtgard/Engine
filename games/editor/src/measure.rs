//! Measuring in the scene view (the Measure tool, M): drag from one point to another and the
//! distance shows on the line, with how far it runs along each axis, and the rise drawn as a
//! right triangle when the ends differ in height. Ends snap to corners near the pointer, so
//! "how wide is this doorway" measures corner to corner (Unreal measures with the middle mouse in
//! orthographic views; Blender's Measure tool snaps with Ctrl). The ruler stays until the next
//! one, and draws while the tool is active.

use engine::prelude::*;

use crate::editor::Editor;
use crate::view::ViewInfo;

/// How near the pointer a corner has to be for an end to snap to it, points.
const SNAP: f32 = 14.0;

/// The ruler.
#[derive(Default)]
pub struct Measure {
    pub from: Option<DVec3>,
    pub to: Option<DVec3>,
    dragging: bool,
}

/// What an end would land on under `pos`: a corner near it, a surface, or the ground plane (or,
/// looking level, the plane through the world origin facing the camera).
pub fn point_under(world: &mut World, view: &ViewInfo, pos: egui::Pos2) -> Option<(DVec3, bool)> {
    if let Some(corner) = crate::snap::corner_near(world, view, pos, SNAP) {
        return Some((corner, true));
    }
    let (o, d) = view.ray(pos);
    if let Some(hit) = crate::picking::pick_surface(world, o, d, 10_000.0, &|_| false) {
        return Some((hit, false));
    }
    let dir = d.as_dvec3();
    let normal = if dir.y.abs() > 0.05 { DVec3::Y } else { -dir };
    let t = -o.dot(normal) / dir.dot(normal);
    (t > 0.0 && t.is_finite()).then(|| (o + dir * t, false))
}

/// "3.42 m", "42 cm", "1.25 km".
pub fn format_length(m: f64) -> String {
    let a = m.abs();
    if a >= 1000.0 {
        format!("{:.2} km", m / 1000.0)
    } else if a < 1.0 {
        format!("{:.1} cm", m * 100.0)
    } else {
        format!("{m:.2} m")
    }
}

/// The tool in the scene view: drag to measure, hover to see where an end would land.
pub fn brush(
    ui: &egui::Ui,
    world: &mut World,
    editor: &mut Editor,
    view: &ViewInfo,
    response: &egui::Response,
) {
    let pos = response.hover_pos().or(response.interact_pointer_pos());
    let under = pos.and_then(|p| point_under(world, view, p));
    // A drag counts as one once the pointer has moved, so it starts where the button went down.
    if response.drag_started_by(egui::PointerButton::Primary)
        && let Some(origin) = ui.input(|i| i.pointer.press_origin())
        && let Some((at, _)) = point_under(world, view, origin)
    {
        let m = &mut editor.measure;
        m.from = Some(at);
        m.to = Some(at);
        m.dragging = true;
    }
    let m = &mut editor.measure;
    if m.dragging {
        if let Some((at, _)) = under {
            m.to = Some(at);
        }
        if !ui.input(|i| i.pointer.primary_down()) {
            m.dragging = false;
        }
    }
    if let Some((at, corner)) = under
        && let Some(p) = view.project(at)
    {
        let color = if corner {
            egui::Color32::from_rgb(0xff, 0xd8, 0x40)
        } else {
            egui::Color32::from_white_alpha(200)
        };
        ui.painter().circle_stroke(
            p,
            if corner { 6.0 } else { 4.0 },
            egui::Stroke::new(1.5, color),
        );
    }
    draw(ui, world, editor, view);
}

/// The ruler: its line (over everything), the rise triangle, and the lengths.
fn draw(ui: &egui::Ui, world: &mut World, editor: &Editor, view: &ViewInfo) {
    let (Some(a), Some(b)) = (editor.measure.from, editor.measure.to) else {
        return;
    };
    let d = b - a;
    let mut debug = world.resource_mut::<DebugDraw>();
    debug.line(a, b, [1.0, 0.85, 0.25, 1.0]).on_top();
    if d.y.abs() > 0.01 && (d.x.abs() > 0.01 || d.z.abs() > 0.01) {
        // The rise as a right triangle: across at the lower end's height, then up.
        let (low, high) = if d.y > 0.0 { (a, b) } else { (b, a) };
        let corner = DVec3::new(high.x, low.y, high.z);
        debug.line(low, corner, [1.0, 1.0, 1.0, 0.45]).on_top();
        debug.line(corner, high, [0.45, 0.95, 0.5, 0.7]).on_top();
    }
    let painter = ui.painter();
    for end in [a, b] {
        if let Some(p) = view.project(end) {
            painter.circle_filled(p, 4.0, egui::Color32::from_rgb(0xff, 0xd8, 0x40));
        }
    }
    let Some(mid) = view.project((a + b) * 0.5) else {
        return;
    };
    let main = format_length(d.length());
    let parts = format!(
        "X {}  ·  Y {}  ·  Z {}  ·  flat {}",
        format_length(d.x.abs()),
        format_length(d.y.abs()),
        format_length(d.z.abs()),
        format_length(DVec3::new(d.x, 0.0, d.z).length()),
    );
    let big = painter.layout_no_wrap(
        main,
        egui::FontId::proportional(17.0),
        egui::Color32::from_rgb(0xff, 0xe0, 0x70),
    );
    let small = painter.layout_no_wrap(
        parts,
        egui::FontId::proportional(11.5),
        egui::Color32::from_white_alpha(215),
    );
    let width = big.size().x.max(small.size().x);
    let size = egui::vec2(width + 16.0, big.size().y + small.size().y + 10.0);
    let card = egui::Rect::from_center_size(mid + egui::vec2(0.0, -size.y * 0.5 - 10.0), size);
    painter.rect_filled(
        card,
        5,
        egui::Color32::from_rgba_unmultiplied(22, 23, 26, 225),
    );
    painter.galley(
        egui::pos2(card.center().x - big.size().x * 0.5, card.min.y + 4.0),
        big.clone(),
        egui::Color32::WHITE,
    );
    painter.galley(
        egui::pos2(
            card.center().x - small.size().x * 0.5,
            card.min.y + 6.0 + big.size().y,
        ),
        small,
        egui::Color32::WHITE,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lengths_read_in_sensible_units() {
        assert_eq!(format_length(3.4167), "3.42 m");
        assert_eq!(format_length(0.42), "42.0 cm");
        assert_eq!(format_length(1250.0), "1.25 km");
    }
}
