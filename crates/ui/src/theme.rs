//! [`Theme`]: the palette and the named style classes every screen is built from.
//!
//! Change the look of every screen at once by editing the palette
//! (`ui.theme {"palette": {"accent": "#7fd1ff"}}`) or a class
//! (`resource.patch Theme {"classes": {"button": {"base": {"radius": 2}}}}`).

use std::collections::BTreeMap;

use bevy_ecs::prelude::*;
use serde::{Deserialize, Serialize};

use crate::color::Color;
use crate::style::{Align, Corners, Direction, Fill, Justify, Overflow, Pick, Shadow, StylePatch, Val, pct};

/// Named colors. Classes are generated from these by [`Theme::from_palette`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Palette {
    /// Panel fill, top and bottom of its gradient.
    pub panel_top: Color,
    pub panel_bottom: Color,
    /// Hairline borders on panels and cards.
    pub line: Color,
    /// Cards, slots and buttons.
    pub surface: Color,
    pub surface_hover: Color,
    /// Wells inside panels (slots, bar tracks).
    pub well: Color,
    pub text: Color,
    pub text_dim: Color,
    pub text_faint: Color,
    /// Primary accent (gold) and its light/dark variants.
    pub accent: Color,
    pub accent_light: Color,
    pub accent_dark: Color,
    /// Secondary accent (cyan): information, maps, sci-fi readouts.
    pub info: Color,
    pub good: Color,
    pub bad: Color,
    pub warn: Color,
    /// Item rarity colors: common, uncommon, rare, epic, legendary.
    pub rarity: [Color; 5],
}

impl Default for Palette {
    fn default() -> Self {
        Self {
            panel_top: Color::hexa(0x161e2ef0),
            panel_bottom: Color::hexa(0x0b1019f2),
            line: Color::hexa(0xffffff1c),
            surface: Color::hexa(0x1a2234e6),
            surface_hover: Color::hexa(0x253049f0),
            well: Color::hexa(0x070a12b8),
            text: Color::hex(0xeceff5),
            text_dim: Color::hex(0xa3acbe),
            text_faint: Color::hex(0x687286),
            accent: Color::hex(0xe8b65a),
            accent_light: Color::hex(0xf7d999),
            accent_dark: Color::hex(0x9c7330),
            info: Color::hex(0x5fd0ff),
            good: Color::hex(0x6fdc8c),
            bad: Color::hex(0xff6b6b),
            warn: Color::hex(0xffb648),
            rarity: [
                Color::hex(0xc4c9d4),
                Color::hex(0x6fdc8c),
                Color::hex(0x5aa8ff),
                Color::hex(0xb77dff),
                Color::hex(0xffa645),
            ],
        }
    }
}

/// One class: a base patch and per-state patches.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ClassStyle {
    pub base: StylePatch,
    pub hover: StylePatch,
    pub pressed: StylePatch,
    pub focused: StylePatch,
    pub selected: StylePatch,
    pub disabled: StylePatch,
}

impl ClassStyle {
    fn base(base: StylePatch) -> Self {
        Self {
            base,
            ..Default::default()
        }
    }
}

/// Palette, default text size and the class table.
#[derive(Resource, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Theme {
    pub palette: Palette,
    /// Body text size, logical px.
    pub font_size: f32,
    pub classes: BTreeMap<String, ClassStyle>,
}

impl Default for Theme {
    fn default() -> Self {
        Self::from_palette(Palette::default())
    }
}

impl Theme {
    pub fn class(&self, name: &str) -> Option<&ClassStyle> {
        self.classes.get(name)
    }

    /// Rarity color for tier 0..4 (clamped).
    pub fn rarity(&self, tier: usize) -> Color {
        self.palette.rarity[tier.min(4)]
    }

    /// The default class table built from `p`.
    pub fn from_palette(p: Palette) -> Self {
        let mut c = BTreeMap::new();
        let s = StylePatch::default;
        let soft_shadow = Shadow::new(Color::hexa(0x00000099), 36.0, 14.0);

        c.insert(
            "screen".into(),
            ClassStyle::base(s().fill_parent().pick(Pick::Ignore)),
        );
        c.insert(
            "panel".into(),
            ClassStyle::base(
                s().column()
                    .background(Fill::vertical(p.panel_top, p.panel_bottom))
                    .border(1.0, p.line)
                    .radius(Corners::all(16.0))
                    .shadow(soft_shadow)
                    .backdrop_blur(20.0)
                    .padding(20.0)
                    .gap(14.0)
                    .pick(Pick::Block),
            ),
        );
        c.insert(
            "glass".into(),
            ClassStyle::base(
                s().background(Fill::vertical(
                    p.panel_top.fade(0.75),
                    p.panel_bottom.fade(0.8),
                ))
                .border(1.0, p.line)
                .radius(Corners::all(12.0))
                .backdrop_blur(14.0)
                .shadow(Shadow::new(Color::hexa(0x00000066), 20.0, 6.0)),
            ),
        );
        c.insert(
            "card".into(),
            ClassStyle {
                base: s()
                    .background(p.surface.fade(0.7))
                    .border(1.0, p.line.fade(0.7))
                    .radius(Corners::all(10.0))
                    .padding(12.0)
                    .gap(10.0)
                    .transition(0.12),
                hover: s().background(p.surface_hover).border_color(p.line.scale(1.6)),
                selected: s()
                    .background(Fill::vertical(
                        p.accent.fade(0.16),
                        p.accent.fade(0.05),
                    ))
                    .border_color(p.accent.fade(0.85))
                    .shadow(Shadow::glow(p.accent.fade(0.25), 14.0)),
                disabled: s().opacity(0.5),
                ..Default::default()
            },
        );
        let button_base = s()
            .row()
            .center()
            .padding([10.0, 18.0])
            .gap(8.0)
            .radius(Corners::all(9.0))
            .background(Fill::vertical(p.surface_hover, p.surface))
            .border(1.0, p.line.scale(1.3))
            .color(p.text)
            .font_size(15.0)
            .font_weight(600)
            .shadow(Shadow::new(Color::hexa(0x00000059), 8.0, 3.0))
            .transition(0.12);
        c.insert(
            "button".into(),
            ClassStyle {
                base: button_base,
                hover: s()
                    .background(Fill::vertical(
                        p.surface_hover.scale(1.45),
                        p.surface_hover,
                    ))
                    .border_color(p.accent.fade(0.55))
                    .translate([0.0, -1.0]),
                pressed: s()
                    .scale(0.97)
                    .translate([0.0, 0.0])
                    .background(p.surface),
                focused: s()
                    .border_color(p.accent)
                    .shadow(Shadow::glow(p.accent.fade(0.45), 12.0)),
                selected: s().border_color(p.accent).color(p.accent_light),
                disabled: s().opacity(0.42),
            },
        );
        c.insert(
            "primary".into(),
            ClassStyle {
                base: s()
                    .background(Fill::vertical(p.accent_light, p.accent))
                    .border_color(p.accent_light.with_alpha(0.9))
                    .color(Color::hex(0x23180a))
                    .shadow(Shadow::new(p.accent.fade(0.35), 16.0, 4.0)),
                hover: s()
                    .background(Fill::vertical(
                        p.accent_light.scale(1.12),
                        p.accent.scale(1.12),
                    ))
                    .border_color(Color::WHITE.fade(0.9))
                    .shadow(Shadow::new(p.accent.fade(0.55), 22.0, 6.0)),
                pressed: s().background(Fill::vertical(p.accent, p.accent_dark)),
                ..Default::default()
            },
        );
        c.insert(
            "danger".into(),
            ClassStyle {
                base: s()
                    .background(Fill::vertical(p.bad.scale(0.55), p.bad.scale(0.32)))
                    .border_color(p.bad.fade(0.7)),
                hover: s().background(Fill::vertical(p.bad.scale(0.75), p.bad.scale(0.45))),
                ..Default::default()
            },
        );
        c.insert(
            "ghost".into(),
            ClassStyle {
                base: s()
                    .background(Fill::None)
                    .border_color(Color::TRANSPARENT)
                    .shadow(Shadow::default())
                    .color(p.text_dim),
                hover: s()
                    .background(Color::WHITE.fade(0.06))
                    .color(p.text)
                    .border_color(Color::TRANSPARENT),
                ..Default::default()
            },
        );
        c.insert(
            "icon_button".into(),
            ClassStyle::base(s().padding(8.0).width(36.0).height(36.0).radius(Corners::all(18.0))),
        );
        c.insert(
            "tab".into(),
            ClassStyle {
                base: s()
                    .row()
                    .center()
                    .gap(6.0)
                    .padding([8.0, 14.0])
                    .radius(Corners::all(8.0))
                    .color(p.text_dim)
                    .font_size(14.0)
                    .font_weight(600)
                    .border(1.0, Color::TRANSPARENT)
                    .transition(0.12),
                hover: s().color(p.text).background(Color::WHITE.fade(0.05)),
                selected: s()
                    .color(p.accent_light)
                    .background(Fill::vertical(p.accent.fade(0.18), p.accent.fade(0.06)))
                    .border_color(p.accent.fade(0.5)),
                focused: s().border_color(p.accent),
                ..Default::default()
            },
        );
        c.insert(
            "slot".into(),
            ClassStyle {
                base: s()
                    .width(64.0)
                    .height(64.0)
                    .center()
                    .padding(6.0)
                    .radius(Corners::all(10.0))
                    .background(Fill::Radial {
                        inner: p.surface.fade(0.55),
                        outer: p.well,
                    })
                    .border(1.0, p.line)
                    .transition(0.1),
                hover: s()
                    .border_color(Color::WHITE.fade(0.35))
                    .background(Fill::Radial {
                        inner: p.surface_hover,
                        outer: p.well,
                    }),
                selected: s()
                    .border_color(p.accent)
                    .shadow(Shadow::glow(p.accent.fade(0.4), 12.0)),
                focused: s()
                    .border_color(p.accent_light)
                    .shadow(Shadow::glow(p.accent.fade(0.5), 14.0)),
                ..Default::default()
            },
        );
        c.insert(
            "divider".into(),
            ClassStyle::base(
                s().height(1.0)
                    .width(pct(100.0))
                    .shrink(0.0)
                    .background(Fill::Linear {
                        from: p.line.fade(0.0),
                        to: p.line.scale(1.4),
                        angle_deg: 0.0,
                    }),
            ),
        );
        c.insert(
            "chip".into(),
            ClassStyle::base(
                s().row()
                    .center()
                    .gap(5.0)
                    .padding([3.0, 9.0])
                    .radius(Corners::all(999.0))
                    .background(Color::WHITE.fade(0.07))
                    .border(1.0, p.line)
                    .font_size(12.0)
                    .font_weight(600)
                    .color(p.text_dim),
            ),
        );
        c.insert(
            "kbd".into(),
            ClassStyle::base(
                s().center()
                    .min_width(22.0)
                    .height(22.0)
                    .padding([0.0, 6.0])
                    .radius(Corners::all(5.0))
                    .background(Fill::vertical(
                        Color::WHITE.fade(0.16),
                        Color::WHITE.fade(0.07),
                    ))
                    .border(1.0, Color::WHITE.fade(0.28))
                    .font_size(12.0)
                    .font_weight(700)
                    .color(p.text),
            ),
        );
        c.insert(
            "caption".into(),
            ClassStyle::base(s().font_size(12.0).font_weight(700).color(p.text_faint)),
        );
        c.insert(
            "title".into(),
            ClassStyle::base(s().font_size(26.0).font_weight(700).color(p.text)),
        );
        c.insert(
            "heading".into(),
            ClassStyle::base(s().font_size(18.0).font_weight(700).color(p.text)),
        );
        c.insert(
            "body".into(),
            ClassStyle::base(s().font_size(15.0).font_weight(400).color(p.text_dim)),
        );
        c.insert(
            "dim".into(),
            ClassStyle::base(s().color(p.text_dim)),
        );
        c.insert(
            "accent".into(),
            ClassStyle::base(s().color(p.accent)),
        );
        c.insert(
            "bar_track".into(),
            ClassStyle::base(
                s().height(10.0)
                    .radius(Corners::all(5.0))
                    .background(p.well)
                    .border(1.0, p.line)
                    .overflow(Overflow::Hidden)
                    .padding(1.0),
            ),
        );
        c.insert(
            "bar_fill".into(),
            ClassStyle::base(s().height(pct(100.0)).radius(Corners::all(4.0))),
        );
        c.insert(
            "tooltip".into(),
            ClassStyle::base(
                s().column()
                    .gap(6.0)
                    .padding([12.0, 14.0])
                    .max_width(340.0)
                    .radius(Corners::all(10.0))
                    .background(Fill::vertical(
                        Color::hexa(0x141b2afa),
                        Color::hexa(0x0a0e17fc),
                    ))
                    .border(1.0, p.line.scale(1.4))
                    .shadow(Shadow::new(Color::hexa(0x000000aa), 24.0, 8.0))
                    .pick(Pick::Ignore)
                    .z_index(100),
            ),
        );
        c.insert(
            "drag_ghost".into(),
            ClassStyle::base(
                s().radius(Corners::all(10.0))
                    .background(Fill::vertical(
                        Color::hexa(0x2a3550e8),
                        Color::hexa(0x141b2af0),
                    ))
                    .border(1.5, p.accent.with_alpha(0.9))
                    .shadow(Shadow::new(Color::hexa(0x000000b0), 22.0, 10.0))
                    .scale(1.08)
                    .rotate_deg(-3.0)
                    .opacity(0.92)
                    .pick(Pick::Ignore),
            ),
        );
        c.insert(
            "row".into(),
            ClassStyle::base(s().row().align_items(Align::Center).gap(8.0)),
        );
        c.insert(
            "col".into(),
            ClassStyle::base(s().column().gap(8.0)),
        );
        c.insert(
            "spacer".into(),
            ClassStyle::base(s().grow(1.0)),
        );
        c.insert(
            "scroll".into(),
            ClassStyle::base(
                s().overflow(Overflow::Scroll)
                    .direction(Direction::Column)
                    .min_height(Val::Px(0.0)),
            ),
        );
        c.insert(
            "center".into(),
            ClassStyle::base(s().justify(Justify::Center).align_items(Align::Center)),
        );
        widget_classes(&p, &mut c);
        Self {
            palette: p,
            font_size: 16.0,
            classes: c,
        }
    }
}

/// Classes used by the built-in widgets and screens.
fn widget_classes(p: &Palette, c: &mut BTreeMap<String, ClassStyle>) {
    let s = StylePatch::default;
    let mut add = |name: &str, style: ClassStyle| {
        c.insert(name.to_string(), style);
    };
    add(
        "header_icon",
        ClassStyle::base(
            s().center()
                .width(44.0)
                .height(44.0)
                .radius(Corners::all(12.0))
                .background(Fill::vertical(p.accent.fade(0.28), p.accent.fade(0.08)))
                .border(1.0, p.accent.fade(0.5))
                .color(p.accent_light),
        ),
    );
    add(
        "tab_bar",
        ClassStyle::base(
            s().row()
                .gap(4.0)
                .padding(4.0)
                .radius(Corners::all(11.0))
                .background(p.well)
                .border(1.0, p.line),
        ),
    );
    add(
        "currency",
        ClassStyle::base(s().font_weight(700).color(p.accent_light)),
    );
    add(
        "stat_row",
        ClassStyle::base(
            s().row()
                .align_items(Align::Center)
                .gap(8.0)
                .padding([5.0, 0.0])
                .font_size(14.0),
        ),
    );
    add(
        "stat_value",
        ClassStyle::base(s().font_weight(600).color(p.text)),
    );
    add(
        "setting_row",
        ClassStyle {
            base: s()
                .row()
                .align_items(Align::Center)
                .gap(16.0)
                .padding([12.0, 16.0])
                .min_height(52.0)
                .radius(Corners::all(10.0))
                .background(Color::WHITE.fade(0.03))
                .border(1.0, Color::TRANSPARENT)
                .font_size(15.0)
                .transition(0.1),
            hover: s().background(Color::WHITE.fade(0.06)),
            focused: s()
                .border_color(p.accent.fade(0.8))
                .background(p.accent.fade(0.08)),
            ..Default::default()
        },
    );
    add(
        "slider",
        ClassStyle {
            base: s().radius(Corners::all(8.0)).border(1.0, Color::TRANSPARENT),
            focused: s().border_color(p.accent.fade(0.6)),
            ..Default::default()
        },
    );
    add(
        "pad_button",
        ClassStyle::base(
            s().center()
                .width(22.0)
                .height(22.0)
                .radius(Corners::all(11.0))
                .border(1.0, Color::WHITE.fade(0.35))
                .font_size(12.0)
                .font_weight(700)
                .color(Color::WHITE)
                .shadow(Shadow::new(Color::hexa(0x00000080), 6.0, 2.0)),
        ),
    );
    add(
        "toast",
        ClassStyle::base(
            s().row()
                .align_items(Align::Center)
                .gap(12.0)
                .padding([12.0, 16.0])
                .min_width(280.0)
                .max_width(420.0)
                .radius(Corners::all(12.0))
                .background(Fill::Linear {
                    from: p.panel_top,
                    to: p.panel_bottom,
                    angle_deg: 0.0,
                })
                .border(1.0, p.line.scale(1.3))
                .backdrop_blur(12.0)
                .shadow(Shadow::new(Color::hexa(0x00000088), 22.0, 8.0))
                .transition(0.2),
        ),
    );
    add(
        "hud_panel",
        ClassStyle::base(
            s().background(Fill::vertical(
                Color::hexa(0x0c121ccc),
                Color::hexa(0x070b12d8),
            ))
            .border(1.0, Color::WHITE.fade(0.1))
            .radius(Corners::all(12.0))
            .backdrop_blur(10.0)
            .shadow(Shadow::new(Color::hexa(0x00000066), 16.0, 4.0)),
        ),
    );
    add(
        "badge",
        ClassStyle::base(
            s().center()
                .min_width(18.0)
                .height(18.0)
                .padding([0.0, 5.0])
                .radius(Corners::all(9.0))
                .background(p.bad)
                .font_size(11.0)
                .font_weight(700)
                .color(Color::WHITE),
        ),
    );
    add(
        "slot_count",
        ClassStyle::base(
            s().position(crate::style::Position::Absolute)
                .right(6.0)
                .bottom(3.0)
                .font_size(13.0)
                .font_weight(700)
                .color(p.text)
                .pick(Pick::Ignore),
        ),
    );
    add(
        "label_caps",
        ClassStyle::base(s().font_size(11.0).font_weight(700).color(p.text_faint)),
    );
    add(
        "list_item",
        ClassStyle {
            base: s()
                .row()
                .align_items(Align::Center)
                .gap(12.0)
                .padding([10.0, 12.0])
                .radius(Corners::all(10.0))
                .border(1.0, Color::TRANSPARENT)
                .transition(0.1),
            hover: s().background(Color::WHITE.fade(0.06)),
            focused: s()
                .background(p.accent.fade(0.1))
                .border_color(p.accent.fade(0.7)),
            selected: s()
                .background(Fill::Linear {
                    from: p.accent.fade(0.2),
                    to: p.accent.fade(0.02),
                    angle_deg: 0.0,
                })
                .border_color(p.accent.fade(0.45)),
            disabled: s().opacity(0.45),
            ..Default::default()
        },
    );
    add(
        "dialogue_box",
        ClassStyle::base(
            s().column()
                .gap(10.0)
                .padding([22.0, 30.0, 20.0, 30.0])
                .radius(Corners::all(18.0))
                .background(Fill::vertical(
                    Color::hexa(0x121a28ee),
                    Color::hexa(0x080c14f4),
                ))
                .border(1.0, p.accent.fade(0.32))
                .backdrop_blur(14.0)
                .shadow(Shadow::new(Color::hexa(0x000000a0), 40.0, 10.0)),
        ),
    );
    add(
        "choice",
        ClassStyle {
            base: s().padding([11.0, 14.0]).font_size(17.0).color(p.text_dim),
            hover: s().color(p.text).background(Fill::Linear {
                from: p.accent.fade(0.14),
                to: p.accent.fade(0.0),
                angle_deg: 0.0,
            }),
            focused: s().color(p.accent_light).background(Fill::Linear {
                from: p.accent.fade(0.2),
                to: p.accent.fade(0.02),
                angle_deg: 0.0,
            }),
            ..Default::default()
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_round_trips_through_json() {
        let t = Theme::default();
        let v = serde_json::to_value(&t).unwrap();
        let back: Theme = serde_json::from_value(v).unwrap();
        assert_eq!(back.classes.len(), t.classes.len());
        assert_eq!(back.palette, t.palette);
    }

    #[test]
    fn button_has_state_styles() {
        let t = Theme::default();
        let b = t.class("button").unwrap();
        assert!(!b.hover.is_empty());
        assert!(!b.focused.is_empty());
        assert!(!b.disabled.is_empty());
    }
}
