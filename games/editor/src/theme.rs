//! The editor's look: a dark, low-contrast theme in the style of current AAA editors, with one
//! blue accent for selection and focus, and axis colors that match the gizmo.

use egui::{Color32, CornerRadius, FontFamily, FontId, Margin, Stroke, TextStyle};
use engine::prelude::*;

pub const BG: Color32 = Color32::from_rgb(0x14, 0x15, 0x17);
pub const PANEL: Color32 = Color32::from_rgb(0x1d, 0x1e, 0x21);
pub const WIDGET: Color32 = Color32::from_rgb(0x2a, 0x2b, 0x2f);
pub const HOVER: Color32 = Color32::from_rgb(0x34, 0x36, 0x3b);
pub const BORDER: Color32 = Color32::from_rgb(0x0c, 0x0c, 0x0d);
pub const TEXT: Color32 = Color32::from_rgb(0xd4, 0xd6, 0xda);
pub const TEXT_DIM: Color32 = Color32::from_rgb(0x8a, 0x8e, 0x96);
pub const ACCENT: Color32 = Color32::from_rgb(0x3d, 0x8b, 0xfd);
pub const ACCENT_DIM: Color32 = Color32::from_rgb(0x1f, 0x4a, 0x8a);
pub const PLAY: Color32 = Color32::from_rgb(0x4c, 0xc2, 0x6a);
pub const WARN: Color32 = Color32::from_rgb(0xf0, 0x7a, 0x4a);
/// X, Y, Z, W.
pub const AXIS_COLORS: [Color32; 4] = [
    Color32::from_rgb(0xf0, 0x4e, 0x5e),
    Color32::from_rgb(0x7b, 0xd1, 0x4a),
    Color32::from_rgb(0x4a, 0x8e, 0xf5),
    Color32::from_rgb(0xb0, 0xb0, 0xb0),
];

/// Apply the theme to an egui context.
pub fn apply(ctx: &egui::Context) {
    ctx.set_theme(egui::Theme::Dark);
    ctx.style_mut_of(egui::Theme::Dark, |style| {
        style.text_styles = [
            (
                TextStyle::Heading,
                FontId::new(16.0, FontFamily::Proportional),
            ),
            (TextStyle::Body, FontId::new(13.0, FontFamily::Proportional)),
            (
                TextStyle::Monospace,
                FontId::new(12.5, FontFamily::Monospace),
            ),
            (
                TextStyle::Button,
                FontId::new(13.0, FontFamily::Proportional),
            ),
            (
                TextStyle::Small,
                FontId::new(10.5, FontFamily::Proportional),
            ),
        ]
        .into();
        let s = &mut style.spacing;
        s.item_spacing = egui::vec2(6.0, 4.0);
        s.button_padding = egui::vec2(7.0, 3.0);
        s.interact_size = egui::vec2(36.0, 20.0);
        s.window_margin = Margin::same(8);
        s.menu_margin = Margin::same(6);
        s.indent = 14.0;
        s.combo_width = 140.0;

        let v = &mut style.visuals;
        v.dark_mode = true;
        v.panel_fill = PANEL;
        v.window_fill = PANEL;
        v.extreme_bg_color = BG;
        v.faint_bg_color = Color32::from_rgb(0x22, 0x23, 0x26);
        v.code_bg_color = BG;
        v.window_stroke = Stroke::new(1.0, BORDER);
        v.window_corner_radius = CornerRadius::same(6);
        v.menu_corner_radius = CornerRadius::same(5);
        v.selection.bg_fill = ACCENT_DIM;
        v.selection.stroke = Stroke::new(1.0, ACCENT);
        v.hyperlink_color = ACCENT;
        v.override_text_color = Some(TEXT);
        v.striped = true;
        v.slider_trailing_fill = true;

        let r = CornerRadius::same(3);
        let w = &mut v.widgets;
        w.noninteractive.bg_fill = PANEL;
        w.noninteractive.weak_bg_fill = PANEL;
        w.noninteractive.bg_stroke = Stroke::new(1.0, Color32::from_rgb(0x2c, 0x2d, 0x31));
        w.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
        w.noninteractive.corner_radius = r;
        w.inactive.bg_fill = WIDGET;
        w.inactive.weak_bg_fill = WIDGET;
        w.inactive.bg_stroke = Stroke::NONE;
        w.inactive.fg_stroke = Stroke::new(1.0, TEXT);
        w.inactive.corner_radius = r;
        w.hovered.bg_fill = HOVER;
        w.hovered.weak_bg_fill = HOVER;
        w.hovered.bg_stroke = Stroke::new(1.0, Color32::from_rgb(0x4a, 0x4d, 0x55));
        w.hovered.fg_stroke = Stroke::new(1.5, Color32::WHITE);
        w.hovered.corner_radius = r;
        w.active.bg_fill = ACCENT_DIM;
        w.active.weak_bg_fill = ACCENT_DIM;
        w.active.bg_stroke = Stroke::new(1.0, ACCENT);
        w.active.fg_stroke = Stroke::new(1.5, Color32::WHITE);
        w.active.corner_radius = r;
        w.open.bg_fill = HOVER;
        w.open.weak_bg_fill = HOVER;
        w.open.corner_radius = r;
    });
}

/// Dimmed helper text.
pub fn dim(text: impl Into<String>) -> egui::RichText {
    egui::RichText::new(text).color(TEXT_DIM)
}

/// A one-character icon for an entity's kind (outliner rows, inspector header).
pub fn entity_icon(world: &World, e: Entity) -> &'static str {
    if world.get::<crate::demos::ModuleDemo>(e).is_some() {
        "💠"
    } else if world.get::<DirectionalLight>(e).is_some() {
        "☀"
    } else if world.get::<PointLight>(e).is_some() {
        "💡"
    } else if world.get::<Camera>(e).is_some() {
        "🎥"
    } else if world.get::<MeshRenderer>(e).is_some() {
        "◼"
    } else if world.get::<Children>(e).is_some() {
        "📁"
    } else {
        "○"
    }
}

/// The dock's style, matched to the theme.
pub fn dock_style(style: &egui::Style) -> egui_dock::Style {
    let mut s = egui_dock::Style::from_egui(style);
    s.main_surface_border_stroke = Stroke::NONE;
    s.main_surface_border_rounding = CornerRadius::ZERO;
    s.separator.width = 2.0;
    s.separator.color_idle = BG;
    s.separator.color_hovered = ACCENT_DIM;
    s.separator.color_dragged = ACCENT;
    s.tab_bar.bg_fill = BG;
    s.tab_bar.height = 26.0;
    s.tab_bar.hline_color = BORDER;
    s.tab.tab_body.bg_fill = PANEL;
    s.tab.tab_body.inner_margin = Margin::symmetric(8, 6);
    s.tab.tab_body.stroke = Stroke::NONE;
    s.tab.active.bg_fill = PANEL;
    s.tab.active.text_color = Color32::WHITE;
    s.tab.active.outline_color = Color32::TRANSPARENT;
    s.tab.inactive.bg_fill = BG;
    s.tab.inactive.text_color = TEXT_DIM;
    s.tab.inactive.outline_color = Color32::TRANSPARENT;
    s.tab.focused.bg_fill = PANEL;
    s.tab.focused.text_color = Color32::WHITE;
    s.tab.focused.outline_color = Color32::TRANSPARENT;
    s.tab.hovered.bg_fill = HOVER;
    s.tab.hovered.text_color = TEXT;
    s.tab.active_with_kb_focus = s.tab.active.clone();
    s.tab.inactive_with_kb_focus = s.tab.inactive.clone();
    s.tab.focused_with_kb_focus = s.tab.focused.clone();
    s.tab.hline_below_active_tab_name = true;
    s
}

#[cfg(test)]
mod tests {
    /// Every non-ASCII character in the editor's string literals must exist in one of egui's
    /// built-in fonts, or it renders as an empty box. (egui's own `has_glyph` reports glyphs
    /// from the emoji font as missing, so this reads the font files.)
    #[test]
    fn every_icon_has_a_glyph() {
        let sources = [
            include_str!("ui.rs"),
            include_str!("viewport.rs"),
            include_str!("inspector.rs"),
            include_str!("theme.rs"),
            include_str!("editor.rs"),
            include_str!("models.rs"),
            include_str!("landscape.rs"),
            include_str!("prefab.rs"),
            include_str!("rigs.rs"),
            include_str!("import.rs"),
            include_str!("camera_preview.rs"),
            include_str!("material_edit.rs"),
            include_str!("materials.rs"),
            include_str!("scatter.rs"),
            include_str!("measure.rs"),
            include_str!("show.rs"),
            include_str!("prefab_ui.rs"),
            include_str!("shots.rs"),
            include_str!("browser.rs"),
            include_str!("content.rs"),
            include_str!("thumbs.rs"),
        ];
        let faces: Vec<_> = [
            epaint_default_fonts::UBUNTU_LIGHT,
            epaint_default_fonts::NOTO_EMOJI_REGULAR,
            epaint_default_fonts::EMOJI_ICON,
        ]
        .into_iter()
        .map(|bytes| ttf_parser::Face::parse(bytes, 0).unwrap())
        .collect();
        let mut missing = std::collections::BTreeSet::new();
        for src in sources {
            for line in src.lines() {
                let code = line.trim_start();
                if code.starts_with("//") {
                    continue;
                }
                let mut in_string = false;
                let mut prev = ' ';
                for c in code.chars() {
                    if c == '"' && prev != '\\' {
                        in_string = !in_string;
                    } else if in_string
                        && !c.is_ascii()
                        && !faces.iter().any(|f| f.glyph_index(c).is_some())
                    {
                        missing.insert(c);
                    }
                    prev = c;
                }
            }
        }
        assert!(missing.is_empty(), "no glyph for: {missing:?}");
    }
}
