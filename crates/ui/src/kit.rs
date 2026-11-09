//! Art-directed widget kits: a theme's classes plus the builders that put them together.
//!
//! [`brass`]: lacquered navy plates, brass bezels with ornate nine-slice corners, parchment
//! cards with aged edges, round medallion buttons, a display serif for names and numbers. A
//! 19th-century harbour and industry look for strategy and city-building games (the
//! Archipelago HUD), built only from the engine's own primitives: materials, bevels, inner
//! shadows, layered drop shadows, corner shapes, frames and eased transitions.

pub mod brass {
    use crate::color::Color;
    use crate::el::El;
    use crate::style::{
        Align, Bevel, CornerShape, Corners, Direction, Ease, Enter, Fill, Frame, Justify, Material,
        MaterialKind, Pick, Shadow, StylePatch,
    };
    use crate::text::{DISPLAY_CAPS_FONT, DISPLAY_FONT, HUMANIST_FONT};
    use crate::theme::{ClassStyle, Theme};

    /// The kit's colors.
    pub struct Colors;

    impl Colors {
        /// Navy lacquer, top and bottom of a plate.
        pub fn lacquer_top() -> Color {
            Color::hex(0x23394a)
        }
        pub fn lacquer_bottom() -> Color {
            Color::hex(0x0d1820)
        }
        pub fn brass() -> Color {
            Color::hex(0xd8ab58)
        }
        pub fn brass_light() -> Color {
            Color::hex(0xf6e0a2)
        }
        pub fn brass_dark() -> Color {
            Color::hex(0x7c5520)
        }
        pub fn paper_top() -> Color {
            Color::hex(0xf1e5c6)
        }
        pub fn paper_bottom() -> Color {
            Color::hex(0xdcc79c)
        }
        /// Text on parchment.
        pub fn ink() -> Color {
            Color::hex(0x2c1f12)
        }
        pub fn ink_dim() -> Color {
            Color::hex(0x6a5339)
        }
        /// Text on lacquer.
        pub fn cream() -> Color {
            Color::hex(0xf3e8cf)
        }
        pub fn cream_dim() -> Color {
            Color::hex(0xb5a684)
        }
        /// Good and bad, readable on both lacquer and parchment.
        pub fn good() -> Color {
            Color::hex(0x7fcf6e)
        }
        pub fn bad() -> Color {
            Color::hex(0xff7b5f)
        }
        pub fn good_ink() -> Color {
            Color::hex(0x2f6e28)
        }
        pub fn bad_ink() -> Color {
            Color::hex(0x9a2f1c)
        }
        pub fn outline() -> Color {
            Color::hex(0x120b04)
        }
    }

    fn s() -> StylePatch {
        StylePatch::default()
    }

    /// Two-layer depth: a tight contact shadow and a wide soft one.
    fn depth(near: f32, far: f32) -> (Shadow, Shadow) {
        (
            Shadow::new(Color::hexa(0x000000b8), near, near * 0.4),
            Shadow::new(Color::hexa(0x00000070), far, far * 0.35),
        )
    }

    /// Add the kit's classes to `theme` (and make its humanist sans the body font, and its
    /// tooltips parchment cards).
    pub fn install(theme: &mut Theme) {
        theme.font_family = HUMANIST_FONT.into();
        theme.font_size = 16.0;
        let c = &mut theme.classes;
        let mut add = |name: &str, style: ClassStyle| {
            c.insert(name.to_string(), style);
        };
        let lacquer = Fill::vertical(Colors::lacquer_top(), Colors::lacquer_bottom());
        let paper = Fill::vertical(Colors::paper_top(), Colors::paper_bottom());

        // A heavy lacquered plate with an ornate brass bezel: the build bar, big panels.
        let (near, far) = depth(10.0, 46.0);
        add(
            "brass_plate",
            ClassStyle {
                base: s()
                    .background(lacquer)
                    .material(Material::new(MaterialKind::Lacquer))
                    .radius(Corners::all(6.0))
                    .border(1.0, Colors::outline())
                    .bevel(Bevel::raised(2.5, 0.14, 0.5).with_line(
                        Colors::brass().with_alpha(0.45),
                        1.0,
                        14.0,
                    ))
                    .shadow(near)
                    .shadow2(far)
                    .frame(Frame::new("frame_brass", 44.0, 30.0).outset(6.0))
                    .padding([20.0, 26.0])
                    .color(Colors::cream())
                    .pick(Pick::Block)
                    .enter(Enter::rise(0.45, 40.0).ease(Ease::Out)),
                ..Default::default()
            },
        );
        // The top strip: lacquer with a thin brass bezel.
        let (near, far) = depth(6.0, 30.0);
        add(
            "brass_bar",
            ClassStyle::base(
                s().background(lacquer)
                    .material(Material::new(MaterialKind::Lacquer).seed(4.0))
                    .radius(Corners::all(5.0))
                    .border(1.0, Colors::outline())
                    .bevel(Bevel::raised(2.0, 0.16, 0.5).with_line(
                        Colors::brass().with_alpha(0.35),
                        1.0,
                        6.0,
                    ))
                    .shadow(near)
                    .shadow2(far)
                    .frame(Frame::new("frame_brass_thin", 16.0, 14.0).outset(3.0))
                    .color(Colors::cream())
                    .pick(Pick::Block)
                    .enter(Enter::rise(0.4, -30.0)),
            ),
        );
        // Parchment: building cards, tooltips, ledgers.
        let (near, far) = depth(8.0, 34.0);
        add(
            "parchment",
            ClassStyle::base(
                s().background(paper)
                    .material(Material::new(MaterialKind::Paper).seed(2.0))
                    .radius(Corners::all(3.0))
                    .border(1.0, Color::hex(0x3d2810))
                    .bevel(Bevel::raised(1.5, 0.35, 0.25).with_line(
                        Color::hexa(0x7a552288),
                        1.0,
                        7.0,
                    ))
                    .shadow(near)
                    .shadow2(far)
                    .color(Colors::ink())
                    .pick(Pick::Block),
            ),
        );
        // A plaque with bitten corners, for a name (the island in view).
        add(
            "cartouche",
            ClassStyle::base(
                s().background(paper)
                    .material(Material::new(MaterialKind::Paper).seed(7.0).strength(0.8))
                    .corner_shape(CornerShape::Scoop)
                    .radius(Corners::all(10.0))
                    .border(1.5, Color::hex(0x3d2810))
                    .bevel(Bevel::raised(2.0, 0.4, 0.3).with_line(
                        Colors::brass_dark().with_alpha(0.7),
                        1.0,
                        4.0,
                    ))
                    .shadow(Shadow::new(Color::hexa(0x000000a0), 6.0, 2.0))
                    .color(Colors::ink())
                    .padding([4.0, 22.0])
                    .direction(Direction::Column)
                    .align_items(Align::Center),
            ),
        );
        // A build tile: parchment face in a thin brass frame that lifts toward the hand.
        add(
            "brass_tile",
            ClassStyle {
                base: s()
                    .background(Fill::vertical(Color::hex(0xece0c0), Color::hex(0xcdb88c)))
                    .material(Material::new(MaterialKind::Paper).strength(0.7).scale(0.7))
                    .radius(Corners::all(4.0))
                    .frame(Frame::new("frame_brass_thin", 16.0, 12.0).outset(2.0))
                    .inset_shadow(Shadow::new(Color::hexa(0x4a2e0e70), 12.0, 0.0))
                    .shadow(Shadow::new(Color::hexa(0x000000b0), 8.0, 3.0))
                    .color(Colors::ink())
                    .transition(0.22)
                    .ease(Ease::Back),
                hover: s()
                    .translate([0.0, -7.0])
                    .scale(1.06)
                    .background(Fill::vertical(Color::hex(0xfaf0d4), Color::hex(0xdcc89c)))
                    .shadow(Shadow::new(Color::hexa(0x000000c0), 20.0, 12.0)),
                pressed: s().translate([0.0, -2.0]).scale(0.97),
                selected: s()
                    .frame(Frame::new("frame_brass_selected", 20.0, 15.0).outset(3.0))
                    .background(Fill::vertical(Color::hex(0xfff3cf), Color::hex(0xe8cf92)))
                    .shadow(Shadow::glow(Color::hexa(0xffc85ab0), 22.0))
                    .sheen(0.9)
                    .translate([0.0, -4.0]),
                disabled: s().opacity(0.55),
                ..Default::default()
            },
        );
        // A round medallion button (a brass bezel around a navy enamel disc).
        add(
            "brass_medal",
            ClassStyle {
                base: s()
                    .background(Fill::Radial {
                        inner: Color::hex(0xfcf4de),
                        outer: Color::hex(0xbfa577),
                    })
                    .radius(Corners::all(999.0))
                    .shadow(Shadow::new(Color::hexa(0x000000b0), 8.0, 3.0))
                    .justify(Justify::Center)
                    .align_items(Align::Center)
                    .color(Colors::ink())
                    .transition(0.2)
                    .ease(Ease::Back),
                hover: s()
                    .scale(1.1)
                    .background(Fill::Radial {
                        inner: Color::hex(0xffffff),
                        outer: Color::hex(0xd6bf91),
                    })
                    .shadow(Shadow::new(Color::hexa(0x000000c0), 16.0, 7.0)),
                pressed: s().scale(0.95),
                selected: s()
                    .background(Fill::Radial {
                        inner: Color::hex(0xfff0b8),
                        outer: Color::hex(0xd99a32),
                    })
                    .shadow(Shadow::glow(Color::hexa(0xffcf6ac0), 18.0))
                    .sheen(0.6),
                ..Default::default()
            },
        );
        // A brass push button with chamfered corners.
        add(
            "brass_button",
            ClassStyle {
                base: s()
                    .row()
                    .center()
                    .gap(6.0)
                    .padding([6.0, 16.0])
                    .background(Fill::vertical(Color::hex(0xf2d692), Color::hex(0xa7772d)))
                    .material(Material::new(MaterialKind::Metal).strength(0.8))
                    .corner_shape(CornerShape::Chamfer)
                    .radius(Corners::all(6.0))
                    .border(1.0, Colors::outline())
                    .bevel(Bevel::raised(2.0, 0.55, 0.4))
                    .shadow(Shadow::new(Color::hexa(0x000000a0), 6.0, 2.0))
                    .color(Colors::ink())
                    .font_weight(700)
                    .font_size(15.0)
                    .transition(0.15)
                    .ease(Ease::Out),
                hover: s()
                    .background(Fill::vertical(Color::hex(0xfde7ae), Color::hex(0xc08b39)))
                    .translate([0.0, -1.0])
                    .shadow(Shadow::new(Color::hexa(0x000000b0), 10.0, 4.0)),
                pressed: s()
                    .translate([0.0, 1.0])
                    .bevel(Bevel::sunken(2.0, 0.3, 0.5)),
                ..Default::default()
            },
        );
        // A darker button for second choices and dangerous ones.
        add(
            "brass_button_dark",
            ClassStyle {
                base: s()
                    .row()
                    .center()
                    .gap(6.0)
                    .padding([6.0, 16.0])
                    .background(Fill::vertical(Color::hex(0x3a2a1a), Color::hex(0x1e140a)))
                    .material(Material::new(MaterialKind::Leather).strength(0.8))
                    .corner_shape(CornerShape::Chamfer)
                    .radius(Corners::all(6.0))
                    .border(1.0, Colors::brass_dark())
                    .bevel(Bevel::raised(1.5, 0.15, 0.5))
                    .shadow(Shadow::new(Color::hexa(0x000000a0), 6.0, 2.0))
                    .color(Colors::cream())
                    .font_weight(700)
                    .font_size(15.0)
                    .transition(0.15)
                    .ease(Ease::Out),
                hover: s()
                    .border_color(Colors::brass())
                    .translate([0.0, -1.0])
                    .background(Fill::vertical(Color::hex(0x4d3822), Color::hex(0x271a0c))),
                pressed: s().translate([0.0, 1.0]),
                ..Default::default()
            },
        );
        // A recessed pill for a counter (goods in the warehouse).
        add(
            "brass_well",
            ClassStyle {
                base: s()
                    .row()
                    .align_items(Align::Center)
                    .gap(5.0)
                    .padding([3.0, 10.0, 3.0, 3.0])
                    .background(Fill::vertical(
                        Color::hexa(0x050b10d0),
                        Color::hexa(0x132530d0),
                    ))
                    .radius(Corners::all(999.0))
                    .inset_shadow(Shadow::new(Color::hexa(0x000000e0), 6.0, 2.0))
                    .bevel(Bevel::sunken(1.5, 0.16, 0.0))
                    .border(1.0, Color::hexa(0x00000090))
                    .transition(0.15),
                hover: s()
                    .background(Fill::vertical(
                        Color::hexa(0x0a1820e0),
                        Color::hexa(0x1d3644e0),
                    ))
                    .border_color(Colors::brass().with_alpha(0.6)),
                ..Default::default()
            },
        );
        // A round enamel badge an icon sits on (goods in wells, the card's emblem).
        add(
            "brass_badge",
            ClassStyle::base(
                s().background(Fill::Radial {
                    inner: Color::hex(0xfbf1d6),
                    outer: Color::hex(0xc9b083),
                })
                .radius(Corners::all(999.0))
                .border(1.5, Colors::brass_dark())
                .bevel(Bevel::raised(1.5, 0.5, 0.35))
                .justify(Justify::Center)
                .align_items(Align::Center)
                .padding(3.0)
                .pick(Pick::Ignore),
            ),
        );
        // A sunken progress track and its fill.
        add(
            "brass_track",
            ClassStyle::base(
                s().height(10.0)
                    .radius(Corners::all(5.0))
                    .background(Color::hexa(0x1b120ac0))
                    .inset_shadow(Shadow::new(Color::hexa(0x000000d0), 4.0, 1.5))
                    .border(1.0, Color::hexa(0x3d2810c0))
                    .padding(1.5),
            ),
        );
        add(
            "brass_fill",
            ClassStyle::base(
                s().height(crate::style::pct(100.0))
                    .radius(Corners::all(4.0))
                    .background(Fill::vertical(Color::hex(0xf3d688), Color::hex(0xa5752c)))
                    .bevel(Bevel::raised(1.0, 0.5, 0.3))
                    .transition(0.4)
                    .ease(Ease::Out),
            ),
        );

        // Type.
        add(
            "brass_title",
            ClassStyle::base(
                s().font_family(DISPLAY_FONT)
                    .font_weight(700)
                    .font_size(24.0)
                    .color(Colors::brass_light()),
            ),
        );
        add(
            "brass_number",
            ClassStyle::base(
                s().font_family(DISPLAY_FONT)
                    .font_weight(900)
                    .font_size(26.0)
                    .color(Colors::cream()),
            ),
        );
        add(
            "brass_label",
            ClassStyle::base(
                s().font_family(DISPLAY_CAPS_FONT)
                    .font_weight(700)
                    .font_size(12.0)
                    .color(Colors::brass()),
            ),
        );
        add(
            "ink_title",
            ClassStyle::base(
                s().font_family(DISPLAY_FONT)
                    .font_weight(700)
                    .font_size(22.0)
                    .color(Colors::ink()),
            ),
        );
        add(
            "ink_label",
            ClassStyle::base(
                s().font_family(DISPLAY_CAPS_FONT)
                    .font_weight(700)
                    .font_size(12.0)
                    .color(Colors::ink_dim()),
            ),
        );
        add(
            "ink",
            ClassStyle::base(s().font_size(15.0).color(Colors::ink())),
        );

        // Tooltips as small parchment cards.
        add(
            "tooltip",
            ClassStyle::base(
                s().column()
                    .gap(6.0)
                    .padding([14.0, 18.0])
                    .max_width(360.0)
                    .background(paper)
                    .material(Material::new(MaterialKind::Paper).seed(5.0))
                    .radius(Corners::all(3.0))
                    .border(1.0, Color::hex(0x3d2810))
                    .bevel(Bevel::raised(1.5, 0.35, 0.25).with_line(
                        Color::hexa(0x7a552288),
                        1.0,
                        5.0,
                    ))
                    .frame(Frame::new("frame_brass_thin", 16.0, 11.0).outset(2.0))
                    .shadow(Shadow::new(Color::hexa(0x000000b0), 10.0, 5.0))
                    .shadow2(Shadow::new(Color::hexa(0x00000060), 34.0, 14.0))
                    .color(Colors::ink())
                    .pick(Pick::Ignore)
                    .z_index(100)
                    .enter(Enter::rise(0.18, 8.0)),
            ),
        );
        add(
            "tooltip_title",
            ClassStyle::base(
                s().font_family(DISPLAY_FONT)
                    .font_weight(700)
                    .font_size(19.0)
                    .color(Colors::ink()),
            ),
        );
        add(
            "tooltip_body",
            ClassStyle::base(s().font_size(15.0).color(Color::hex(0x4a3720))),
        );
    }

    /// A lacquered plate with ornate brass corners.
    pub fn plate() -> El {
        El::new().class("brass_plate")
    }

    /// A parchment card.
    pub fn parchment() -> El {
        El::new().class("parchment")
    }

    /// A round medallion button `size` px across with `icon` in its middle.
    pub fn medallion(icon: &str, size: f32, action: &str) -> El {
        El::new()
            .class("brass_medal")
            .on_click(action)
            .with_style(|s| {
                s.size(size, size)
                    .frame(Frame::new("medallion_ring", 48.0, size * 0.5))
                    .padding(size * 0.17)
            })
            .child(
                El::icon(icon).with_style(|s| s.size(size * 0.66, size * 0.66).pick(Pick::Ignore)),
            )
    }

    /// An icon on a round enamel badge `size` px across.
    pub fn badge(icon: &str, size: f32) -> El {
        El::new()
            .class("brass_badge")
            .with_style(|s| s.size(size, size))
            .child(
                El::icon(icon).with_style(|s| s.size(size * 0.78, size * 0.78).pick(Pick::Ignore)),
            )
    }

    /// A recessed counter: a badge with `icon` and a number (`text` may hold a `{binding}`).
    pub fn well(icon: &str, text: &str) -> El {
        El::new()
            .class("brass_well")
            .child(badge(icon, 30.0))
            .child(
                El::text(text)
                    .text_style(|t| t.nowrap().tabular().bold().size(17.0))
                    .with_style(|s| s.color(Colors::cream()).min_width(22.0))
                    .pick_ignore(),
            )
    }

    /// The ornamental flourish, `width` px wide.
    pub fn divider(width: f32) -> El {
        El::icon("ornament_divider").with_style(|s| s.size(width, width / 10.0).pick(Pick::Ignore))
    }

    /// A small-caps label in brass (on lacquer) or ink (on parchment).
    pub fn label(text: &str, on_paper: bool) -> El {
        El::text(text)
            .class(if on_paper { "ink_label" } else { "brass_label" })
            .text_style(|t| t.nowrap())
            .pick_ignore()
    }

    /// A progress bar `width` px wide, `frac` full.
    pub fn meter(width: f32, frac: f32) -> El {
        El::new()
            .class("brass_track")
            .with_style(|s| s.width(width))
            .child(
                El::new()
                    .class("brass_fill")
                    .with_style(|s| s.width(crate::style::pct(frac.clamp(0.0, 1.0) * 100.0))),
            )
    }
}
