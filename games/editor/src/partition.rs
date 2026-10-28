//! The World Partition panel, as Unreal's World Partition window: the level from above, cut into
//! the runtime-grid cells it streams in (`engine::scene_stream`), with every root where it
//! stands, the view and how far it loads. In Edit mode the whole level is in; Play can stream it
//! instead (Stream in Play), so a big level starts with only the cells around the view and the
//! map shows each cell loading, in and leaving as the view moves.
//!
//! Click a root to select it, double-click anywhere to fly the view there (Unreal's minimap
//! double-click), Shift+double-click to play from there, drag to pan and scroll to zoom.

use engine::prelude::*;
use engine::scene_stream::{RuntimeGrid, SceneLayout, ScenePartition, partition, stream_scene};
use engine::world::{CellState, SimulationFocus, StreamedCells};

use crate::editor::{self, Editor, Mode};
use crate::theme;

/// The name the level streams under while playing.
pub const PLAY_SCENE: &str = "editor-play";

/// Seconds between re-cutting the level while editing.
const REFRESH_SECS: f64 = 0.5;

pub struct Partition {
    /// How the level is cut (one grid: its cell size and load distance).
    pub how: ScenePartition,
    /// Play streams the level instead of keeping all of it in.
    pub stream_play: bool,
    /// Play is streaming it now.
    pub streaming: bool,
    /// The cells, the roots' places (with their entities while editing) and when cut.
    layout: Option<SceneLayout>,
    roots: Vec<(Option<Entity>, DVec3)>,
    error: Option<String>,
    cut_at: f64,
    /// The map's middle (x, z) and scale (points per meter); fitted to the level at first.
    center: DVec2,
    zoom: f32,
    fitted: bool,
}

impl Default for Partition {
    fn default() -> Self {
        Self {
            how: ScenePartition::default(),
            stream_play: false,
            streaming: false,
            layout: None,
            roots: Vec::new(),
            error: None,
            cut_at: f64::NEG_INFINITY,
            center: DVec2::ZERO,
            zoom: 1.0,
            fitted: false,
        }
    }
}

impl Partition {
    fn grid(&mut self) -> &mut RuntimeGrid {
        if self.how.grids.is_empty() {
            self.how.grids.push(RuntimeGrid::default());
        }
        &mut self.how.grids[0]
    }

    /// Cut the scene as it stands into cells.
    fn cut(&mut self, world: &mut World) {
        let entities = crate::scene_ops::scene_roots(world);
        let doc = crate::scene_ops::snapshot_scene(world);
        self.roots = entities
            .into_iter()
            .zip(&doc.entities)
            .filter_map(|(e, d)| {
                let t = d.components.get("Transform")?;
                let t = serde_json::from_value::<Transform>(t.clone()).ok()?;
                Some((Some(e), t.translation))
            })
            .collect();
        match partition(PLAY_SCENE, &doc, &self.how) {
            Ok(layout) => {
                self.layout = Some(layout);
                self.error = None;
            }
            Err(e) => self.error = Some(e),
        }
    }

    /// Fit the map to the level.
    fn fit(&mut self, rect: egui::Rect) {
        let Some(layout) = &self.layout else {
            return;
        };
        let (mut lo, mut hi) = (DVec2::splat(f64::INFINITY), DVec2::splat(f64::NEG_INFINITY));
        for c in &layout.cells {
            let min = DVec2::new(c.x as f64, c.z as f64) * c.size;
            lo = lo.min(min);
            hi = hi.max(min + c.size);
        }
        if !lo.x.is_finite() {
            return;
        }
        self.center = (lo + hi) * 0.5;
        let size = (hi - lo).max(DVec2::splat(16.0));
        self.zoom = ((rect.width() - 24.0) / size.x as f32)
            .min((rect.height() - 24.0) / size.y as f32)
            .clamp(0.01, 50.0);
        self.fitted = true;
    }
}

/// Play streams the level: it streams under [`PLAY_SCENE`] and the scene's own entities go
/// until Stop puts them back (Play's snapshot has them). False if it can't stream.
pub fn start_streaming(world: &mut World, editor: &mut Editor) -> bool {
    let t = editor::now(world);
    let doc = crate::scene_ops::snapshot_scene(world);
    match stream_scene(world, PLAY_SCENE, &doc, &editor.partition.how) {
        Ok(layout) => {
            crate::scene_ops::clear_scene(world);
            editor.info(
                t,
                format!(
                    "Play streams the level: {} roots always in, {} cells load around the view",
                    layout.persistent.len(),
                    layout.cells.len()
                ),
            );
            editor.partition.layout = Some(layout);
            editor.partition.streaming = true;
            true
        }
        Err(e) => {
            editor.error(
                t,
                format!("Could not stream the level, playing it whole: {e}"),
            );
            false
        }
    }
}

/// Stop streaming (Stop, before it restores the scene).
pub fn stop_streaming(world: &mut World, editor: &mut Editor) {
    if std::mem::take(&mut editor.partition.streaming) {
        // Its cells unload at once, inside the call, despawning their own entities.
        engine::scene_stream::unstream_scene(world, PLAY_SCENE);
        editor.partition.cut_at = f64::NEG_INFINITY;
    }
}

/// An entity a cell of the streamed level spawned: the streaming takes it down, not Stop.
pub fn is_streamed(world: &World, e: Entity) -> bool {
    let Some(member) = world.get::<engine::world::CellMember>(e) else {
        return false;
    };
    world
        .get_resource::<StreamedCells>()
        .and_then(|c| c.cells.get(member.0.0 as usize))
        .is_some_and(|c| c.key.starts_with(&format!("scene:{PLAY_SCENE}/")))
}

/// A cell's state on the map.
fn state_of(world: &World, key: &str) -> Option<CellState> {
    world
        .get_resource::<StreamedCells>()?
        .cells
        .iter()
        .find(|c| c.key == key)
        .map(|c| c.state)
}

fn state_color(state: Option<CellState>) -> (egui::Color32, &'static str) {
    match state {
        // Editing: everything is in the editor.
        None => (egui::Color32::from_rgb(0x2f, 0x4f, 0x7a), "in the editor"),
        Some(CellState::Loaded) => (egui::Color32::from_rgb(0x3f, 0x9a, 0x55), "loaded"),
        Some(s) if s.is_waking() => (egui::Color32::from_rgb(0xc9, 0x93, 0x3a), "loading"),
        Some(CellState::Unloading) => (egui::Color32::from_rgb(0x8a, 0x5a, 0x2a), "leaving"),
        Some(_) => (egui::Color32::from_rgb(0x2a, 0x2c, 0x31), "out (far proxy)"),
    }
}

/// The panel.
pub fn panel(ui: &mut egui::Ui, world: &mut World, editor: &mut Editor) {
    let now = ui.input(|i| i.time);
    if !editor.partition.streaming && now - editor.partition.cut_at > REFRESH_SECS {
        editor.partition.cut(world);
        editor.partition.cut_at = now;
    }
    let mut recut = false;
    let mut refit = false;
    ui.horizontal(|ui| {
        let p = &mut editor.partition;
        ui.label("Cell");
        recut |= ui
            .add(
                egui::DragValue::new(&mut p.grid().cell_m)
                    .range(8.0..=4096.0)
                    .speed(4.0)
                    .suffix(" m"),
            )
            .on_hover_text("Runtime grid cell size (Unreal's default is 128 m)")
            .changed();
        ui.label("Loads within");
        recut |= ui
            .add(
                egui::DragValue::new(&mut p.grid().load_m)
                    .range(8.0..=20000.0)
                    .speed(4.0)
                    .suffix(" m"),
            )
            .on_hover_text("A cell loads when the view is this close to its edge")
            .changed();
        ui.add_enabled(
            editor.mode == Mode::Edit,
            egui::Checkbox::new(&mut p.stream_play, "Stream in Play"),
        )
        .on_hover_text(
            "Play loads only the cells around the view, as the game will, instead of the whole level",
        );
        refit = ui.button("Fit").on_hover_text("Show the whole level").clicked();
        if let Some(layout) = &p.layout {
            let spatial: usize = layout.cells.iter().map(|c| c.roots.len()).sum();
            let loaded = layout
                .cells
                .iter()
                .filter(|c| state_of(world, &c.key) == Some(CellState::Loaded))
                .count();
            let text = if p.streaming {
                format!("{loaded} of {} cells loaded", layout.cells.len())
            } else {
                format!(
                    "{} cells · {spatial} roots in cells · {} always loaded",
                    layout.cells.len(),
                    layout.persistent.len()
                )
            };
            ui.label(theme::dim(text));
        }
    });
    if recut && !editor.partition.streaming {
        editor.partition.cut(world);
        editor.partition.cut_at = now;
    }
    if let Some(e) = &editor.partition.error {
        ui.colored_label(theme::WARN, e);
    }

    let rect = ui.available_rect_before_wrap();
    let response = ui.allocate_rect(rect, egui::Sense::click_and_drag());
    if refit || !editor.partition.fitted {
        editor.partition.fit(rect);
    }
    let p = &mut editor.partition;
    // Pan and zoom about the pointer.
    if response.dragged() {
        let d = response.drag_delta();
        p.center -= DVec2::new(d.x as f64, d.y as f64) / p.zoom as f64;
    }
    if response.hovered() {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll != 0.0
            && let Some(at) = response.hover_pos()
        {
            let before = to_world(p, rect, at);
            p.zoom = (p.zoom * (scroll * 0.004).exp()).clamp(0.005, 80.0);
            let after = to_world(p, rect, at);
            p.center += before - after;
        }
    }

    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0, theme::BG);
    let (center, zoom) = (p.center, p.zoom);
    let to_screen = |x: f64, z: f64| {
        rect.center()
            + egui::vec2(
                ((x - center.x) * zoom as f64) as f32,
                ((z - center.y) * zoom as f64) as f32,
            )
    };
    let hover = response.hover_pos();
    let mut tip: Option<String> = None;

    // Cells, finest first; coarser levels outlined over them.
    if let Some(layout) = &p.layout {
        let mut cells: Vec<_> = layout.cells.iter().collect();
        cells.sort_by_key(|c| c.level);
        for c in cells {
            let min = to_screen(c.x as f64 * c.size, c.z as f64 * c.size);
            let max = to_screen((c.x + 1) as f64 * c.size, (c.z + 1) as f64 * c.size);
            let r = egui::Rect::from_min_max(min, max);
            if !r.intersects(rect) {
                continue;
            }
            let state = if p.streaming {
                Some(state_of(world, &c.key).unwrap_or_default())
            } else {
                None
            };
            let (color, said) = state_color(state);
            if c.level == 0 {
                painter.rect_filled(r.shrink(0.5), 0, color);
                painter.rect_stroke(
                    r,
                    0,
                    egui::Stroke::new(1.0, theme::BORDER),
                    egui::StrokeKind::Inside,
                );
            } else {
                painter.rect_filled(r.shrink(0.5), 0, color.gamma_multiply(0.35));
                painter.rect_stroke(
                    r,
                    0,
                    egui::Stroke::new(2.0, egui::Color32::from_rgb(0xe8, 0xb8, 0x5a)),
                    egui::StrokeKind::Inside,
                );
            }
            if hover.is_some_and(|h| r.contains(h)) && (tip.is_none() || c.level > 0) {
                tip = Some(format!(
                    "{} cell {}, {} ({:.0} m{}): {} root{}, {said}",
                    c.grid,
                    c.x,
                    c.z,
                    c.size,
                    if c.level > 0 {
                        format!(", level {}", c.level)
                    } else {
                        String::new()
                    },
                    c.roots.len(),
                    if c.roots.len() == 1 { "" } else { "s" },
                ));
            }
        }
    }

    // Roots where they stand; the selection lit.
    let mut picked = None;
    if !p.streaming {
        for &(e, at) in &p.roots {
            let s = to_screen(at.x, at.z);
            if !rect.contains(s) {
                continue;
            }
            let selected = e.is_some_and(|e| editor.selection.contains(&e));
            let (r, color) = if selected {
                (3.5, theme::ACCENT)
            } else {
                (2.0, egui::Color32::from_white_alpha(200))
            };
            painter.circle_filled(s, r, color);
            if let (Some(h), Some(e)) = (hover, e)
                && h.distance(s) < 6.0
            {
                picked = Some(e);
                tip = Some(crate::scene_ops::label(world, e));
            }
        }
    }

    // The view (the streaming source) and how far it loads.
    let mut q = world.query_filtered::<&GlobalTransform, With<SimulationFocus>>();
    let source = q.iter(world).next().map(|g| g.translation());
    let view = world
        .get::<GlobalTransform>(editor.camera)
        .map(|g| g.to_transform());
    if let Some(at) = source {
        let s = to_screen(at.x, at.z);
        let load = p.how.grids.first().map_or(256.0, |g| g.load_m);
        painter.circle_stroke(
            s,
            (load * zoom as f64) as f32,
            egui::Stroke::new(1.0, egui::Color32::from_white_alpha(150)),
        );
    }
    if let Some(t) = view {
        let s = to_screen(t.translation.x, t.translation.z);
        let f = t.forward();
        let dir = egui::vec2(f.x, f.z).normalized() * 14.0;
        if dir.is_finite() {
            let side = egui::vec2(-dir.y, dir.x) * 0.6;
            painter.add(egui::Shape::convex_polygon(
                vec![s, s + dir + side, s + dir - side],
                theme::ACCENT.gamma_multiply(0.6),
                egui::Stroke::NONE,
            ));
        }
        painter.circle_filled(s, 4.0, egui::Color32::WHITE);
    }

    // Scale bar.
    let meters = nice_length(120.0 / zoom as f64);
    let bar = (meters * zoom as f64) as f32;
    let a = rect.left_bottom() + egui::vec2(12.0, -14.0);
    painter.line_segment(
        [a, a + egui::vec2(bar, 0.0)],
        egui::Stroke::new(2.0, theme::TEXT),
    );
    painter.text(
        a + egui::vec2(bar + 6.0, 0.0),
        egui::Align2::LEFT_CENTER,
        if meters >= 1000.0 {
            format!("{} km", meters / 1000.0)
        } else {
            format!("{meters} m")
        },
        egui::FontId::proportional(11.0),
        theme::TEXT,
    );
    // Legend.
    let mut x = rect.right() - 12.0;
    let legend: &[CellState] = if p.streaming {
        &[
            CellState::Unloaded,
            CellState::Unloading,
            CellState::Loading,
            CellState::Loaded,
        ]
    } else {
        &[]
    };
    let items: Vec<(egui::Color32, &str)> = if legend.is_empty() {
        vec![state_color(None)]
    } else {
        legend.iter().map(|&s| state_color(Some(s))).collect()
    };
    for (color, said) in items.into_iter().rev() {
        let galley = painter.layout_no_wrap(
            said.to_string(),
            egui::FontId::proportional(11.0),
            theme::TEXT,
        );
        x -= galley.size().x;
        let y = rect.bottom() - 14.0;
        painter.galley(
            egui::pos2(x, y - galley.size().y * 0.5),
            galley,
            theme::TEXT,
        );
        x -= 14.0;
        painter.rect_filled(
            egui::Rect::from_center_size(egui::pos2(x + 5.0, y), egui::vec2(10.0, 10.0)),
            2,
            color,
        );
        x -= 10.0;
    }

    if let Some(tip) = tip {
        response.clone().on_hover_text_at_pointer(tip);
    }
    if response.clicked()
        && let Some(e) = picked
    {
        editor.select(Some(e));
    }
    if response.double_clicked()
        && let Some(at) = response.interact_pointer_pos()
    {
        // Fly the view over that spot, looking at it as it looks now.
        let target = to_world(&editor.partition, rect, at);
        let point = DVec3::new(target.x, 0.0, target.y);
        if ui.input(|i| i.modifiers.shift) && editor.mode == Mode::Edit {
            // Unreal's Shift+double-click: play from there, streaming the level around it.
            play_at(world, editor, point);
        } else {
            editor.focus = Some((point, 12.0));
        }
    }
}

/// Play with the view over `point`, the level streaming around it (Shift+double-click).
pub fn play_at(world: &mut World, editor: &mut Editor, point: DVec3) {
    if let Some(mut t) = world.get::<Transform>(editor.camera).copied() {
        let back = t.forward().as_dvec3() * 14.0;
        t.translation =
            DVec3::new(point.x, point.y + 6.0, point.z) - DVec3::new(back.x, 0.0, back.z);
        world.entity_mut(editor.camera).insert(t);
    }
    editor.partition.stream_play = true;
    editor::play(world, editor);
}

fn to_world(p: &Partition, rect: egui::Rect, at: egui::Pos2) -> DVec2 {
    let d = at - rect.center();
    p.center + DVec2::new(d.x as f64, d.y as f64) / p.zoom as f64
}

/// A round length near `m`: 1, 2 or 5 times a power of ten.
fn nice_length(m: f64) -> f64 {
    let p = 10f64.powf(m.max(1e-3).log10().floor());
    [1.0, 2.0, 5.0, 10.0]
        .into_iter()
        .map(|k| k * p)
        .rfind(|&v| v <= m)
        .unwrap_or(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scale_bars_use_round_lengths() {
        assert_eq!(nice_length(130.0), 100.0);
        assert_eq!(nice_length(260.0), 200.0);
        assert_eq!(nice_length(7.0), 5.0);
        assert_eq!(nice_length(1000.0), 1000.0);
    }
}
