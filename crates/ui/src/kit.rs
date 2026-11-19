//! Art-directed widget kits: a theme's classes plus the builders that put them together.
//!
//! [`brass`]: lacquered navy plates, brass bezels with ornate nine-slice corners, parchment
//! cards with aged edges, round medallion buttons, a display serif for names and numbers. A
//! 19th-century harbour and industry look for strategy and city-building games (the
//! Archipelago HUD), built only from the engine's own primitives: materials, bevels, inner
//! shadows, layered drop shadows, corner shapes, frames and eased transitions.
//!
//! [`glass`]: chamfered smoked-glass plates with scanlines, neon cyan hairlines and glows and a
//! squared technical face, made by restyling the built-in screens' classes.

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
        // An alert: a slim lacquer strip with a brass hairline, lifted on hover.
        let (near, far) = depth(5.0, 22.0);
        add(
            "brass_alert",
            ClassStyle {
                base: s()
                    .row()
                    .gap(12.0)
                    .align_items(Align::Center)
                    .padding([10.0, 12.0, 10.0, 16.0])
                    .background(lacquer)
                    .material(Material::new(MaterialKind::Lacquer).seed(9.0))
                    .radius(Corners::all(8.0))
                    .border(1.0, Colors::outline())
                    .bevel(Bevel::raised(1.5, 0.12, 0.5).with_line(
                        Colors::brass().with_alpha(0.5),
                        1.0,
                        8.0,
                    ))
                    .shadow(near)
                    .shadow2(far)
                    .color(Colors::cream())
                    .pick(Pick::Block)
                    .transition(0.15)
                    .ease(Ease::Out)
                    .enter(Enter::slide(0.35, -60.0)),
                hover: s().translate([3.0, 0.0]),
                ..Default::default()
            },
        );
        // The engine's toasts as the same lacquer strips.
        let (near, far) = depth(5.0, 22.0);
        add(
            "toast",
            ClassStyle::base(
                s().row()
                    .align_items(Align::Center)
                    .gap(12.0)
                    .padding([10.0, 16.0, 10.0, 16.0])
                    .min_width(300.0)
                    .max_width(440.0)
                    .background(lacquer)
                    .material(Material::new(MaterialKind::Lacquer).seed(11.0))
                    .radius(Corners::all(8.0))
                    .border(1.0, Colors::outline())
                    .bevel(Bevel::raised(1.5, 0.12, 0.5).with_line(
                        Colors::brass().with_alpha(0.5),
                        1.0,
                        8.0,
                    ))
                    .shadow(near)
                    .shadow2(far)
                    .color(Colors::cream())
                    .transition(0.2),
            ),
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

    /// How rare an item is: its socket's rim colour and glow.
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub enum Rank {
        Common,
        Fine,
        Rare,
        Storied,
    }

    impl Rank {
        pub const ALL: [Rank; 4] = [Rank::Common, Rank::Fine, Rank::Rare, Rank::Storied];

        /// Pewter, verdigris green, sapphire and gold.
        pub fn color(self) -> Color {
            match self {
                Rank::Common => Color::hex(0xa8a496),
                Rank::Fine => Color::hex(0x5fa25a),
                Rank::Rare => Color::hex(0x4f86d6),
                Rank::Storied => Color::hex(0xf0b43c),
            }
        }

        pub fn name(self) -> &'static str {
            match self {
                Rank::Common => "Common",
                Rank::Fine => "Fine",
                Rank::Rare => "Rare",
                Rank::Storied => "Storied",
            }
        }
    }

    /// An item socket `size` px across: a sunken brass-ringed hollow, empty (a faint cross, for
    /// "put an item here"), or holding `item` on an enamel badge rimmed and lit in its rank's
    /// colour. Clicking sends `action`.
    pub fn socket(item: Option<(&str, Rank)>, size: f32, action: &str) -> El {
        let hollow = El::new().on_click(action).with_style(move |s| {
            s.size(size, size)
                .radius(Corners::all(999.0))
                .background(Fill::vertical(
                    Color::hexa(0x050b10e0),
                    Color::hexa(0x1a2c38e0),
                ))
                .inset_shadow(Shadow::new(Color::hexa(0x000000f0), size * 0.14, 2.0))
                .border(1.5, Colors::brass_dark())
                .bevel(Bevel::sunken(1.5, 0.2, 0.0))
                .justify(Justify::Center)
                .align_items(Align::Center)
                .transition(0.15)
                .pick(Pick::Block)
        });
        match item {
            None => hollow.child(
                El::text("+")
                    .text_style(move |t| t.bold().size(size * 0.42))
                    .with_style(|s| s.color(Colors::brass().with_alpha(0.45)))
                    .pick_ignore(),
            ),
            Some((icon, rank)) => {
                let c = rank.color();
                let lit = matches!(rank, Rank::Rare | Rank::Storied);
                hollow.child(
                    El::new()
                        .class("brass_badge")
                        .with_style(move |s| {
                            let s = s.size(size * 0.84, size * 0.84).border(2.5, c).shadow(
                                Shadow::new(
                                    c.with_alpha(if lit { 0.75 } else { 0.3 }),
                                    size * if lit { 0.3 } else { 0.12 },
                                    0.0,
                                ),
                            );
                            if rank == Rank::Storied {
                                s.sheen(0.8)
                            } else {
                                s
                            }
                        })
                        .child(El::icon(icon).with_style(move |s| {
                            s.size(size * 0.62, size * 0.62).pick(Pick::Ignore)
                        })),
                )
            }
        }
    }

    /// What an item in a socket is doing, drawn over the socket by [`socket_with`]: a dark
    /// sweep for the wait until it can be pressed again (as on an action bar), an arc filling
    /// around the rim toward a charge, a bright arc that drains while it acts, pips for the
    /// uses left, and a slow pulse when it is ready to press.
    #[derive(
        bevy_ecs::component::Component,
        Clone,
        Debug,
        Default,
        PartialEq,
        serde::Serialize,
        serde::Deserialize,
    )]
    #[serde(default)]
    pub struct SocketState {
        /// 0..1 of the wait still to go (0: none).
        pub cooldown: f32,
        /// 0..1 toward a full charge, shown as an arc around the rim.
        pub fill: Option<f32>,
        /// 0..1 of its acting time still to go.
        pub acting: Option<f32>,
        /// Uses left, of at most `pips_max` (dots along the bottom of the rim).
        pub pips: u32,
        pub pips_max: u32,
        /// Ready to press: the rim pulses.
        pub ready: bool,
        /// Not working yet (a later round, a locked rule): the badge greys out.
        pub inert: bool,
        /// The arcs' colour (the item's rank colour reads best).
        pub color: Color,
        /// A short label over the badge: seconds of wait, or uses left.
        pub label: String,
    }

    /// How much wider than its socket a state overlay is.
    const SOCKET_OVERLAY: f32 = 1.8;

    /// [`socket`] with an item's state drawn over it ([`SocketState`]). `state_id` (or "")
    /// names the overlay, so a game can update its `SocketState` in place every frame
    /// without rebuilding the socket.
    pub fn socket_with(
        item: Option<(&str, Rank)>,
        size: f32,
        action: &str,
        state: &SocketState,
        state_id: &str,
    ) -> El {
        let inert = state.inert;
        let overlay = El::new()
            .paint("brass_socket_state")
            .attach("SocketState", state)
            .with_style(move |s| {
                // Wider than the socket: painters clip to their own box, and the glows and
                // the outer arcs reach past the rim.
                let pad = size * (SOCKET_OVERLAY - 1.0) * 0.5;
                s.position(crate::style::Position::Absolute)
                    .left(-pad)
                    .top(-pad)
                    .size(size * SOCKET_OVERLAY, size * SOCKET_OVERLAY)
                    .pick(Pick::Ignore)
            });
        let overlay = if state_id.is_empty() {
            overlay
        } else {
            overlay.id(state_id)
        };
        let base = socket(item, size, action);
        let base = if inert {
            base.with_style(|s| s.opacity(0.55))
        } else {
            base
        };
        El::new()
            .with_style(move |s| s.size(size, size))
            .child(base)
            .child(overlay)
    }

    pub(crate) fn paint_socket_state(
        world: &mut bevy_ecs::world::World,
        e: bevy_ecs::entity::Entity,
        ctx: &mut crate::draw::DrawCtx,
    ) {
        use engine_core::math::Vec2;
        let Some(st) = world.get::<SocketState>(e).cloned() else {
            return;
        };
        let tau = std::f32::consts::TAU;
        let rect = ctx.rect;
        let c = rect.center();
        let r = rect.w.min(rect.h) * 0.5 / SOCKET_OVERLAY;
        let col = if st.color.a > 0.0 {
            st.color
        } else {
            Colors::brass_light()
        };
        // The wait: a dark pie over the part still to go, its leading edge lit.
        if st.cooldown > 0.001 {
            let cd = st.cooldown.clamp(0.0, 1.0);
            let start = tau * (1.0 - cd);
            let pr = r * 0.84;
            ctx.ring(
                c,
                Vec2::splat(pr * 0.5),
                pr,
                Color::hexa(0x050302d8),
                start,
                tau * cd,
                0.0,
            );
            let edge = Vec2::new(start.sin(), -start.cos());
            ctx.line(c, c + edge * pr, 1.5 * ctx.px, col.with_alpha(0.9), 0.0);
        }
        // Toward a charge: a dim track round the rim and the filled part bright.
        if let Some(f) = st.fill {
            let f = f.clamp(0.0, 1.0);
            let rr = Vec2::splat(r - 2.0 * ctx.px);
            ctx.ring(c, rr, 3.0 * ctx.px, Color::hexa(0x00000080), 0.0, tau, 0.0);
            ctx.ring(c, rr, 3.0 * ctx.px, col, 0.0, tau * f, 0.0);
            if f >= 0.999 {
                ctx.glow(c, r * 1.25, col.with_alpha(0.35), 1.6);
            }
        }
        // Acting: a bright arc draining as its time runs out, and a glow.
        if let Some(a) = st.acting {
            let a = a.clamp(0.0, 1.0);
            let pulse = 0.5 + 0.5 * (ctx.time * 3.0).sin();
            ctx.glow(c, r * 1.3, col.with_alpha(0.22 + 0.12 * pulse), 1.5);
            ctx.ring(
                c,
                Vec2::splat(r + 1.0 * ctx.px),
                3.5 * ctx.px,
                col.lerp(Color::WHITE, 0.35),
                0.0,
                tau * a,
                0.0,
            );
        }
        // Ready to press: the rim breathes.
        if st.ready {
            let pulse = 0.5 + 0.5 * (ctx.time * 2.2).sin();
            ctx.glow(c, r * 1.35, col.with_alpha(0.18 + 0.22 * pulse), 1.4);
            ctx.ring(
                c,
                Vec2::splat(r + 1.0 * ctx.px),
                2.0 * ctx.px,
                col.with_alpha(0.55 + 0.45 * pulse),
                0.0,
                tau,
                0.0,
            );
        }
        // The label: seconds of wait or uses left, outlined so it reads over any badge.
        if !st.label.is_empty() {
            let size = (r * 0.62 / ctx.px).max(11.0);
            for d in [
                Vec2::new(-1.0, 0.0),
                Vec2::new(1.0, 0.0),
                Vec2::new(0.0, -1.0),
                Vec2::new(0.0, 1.0),
                Vec2::new(0.0, 1.5),
            ] {
                ctx.text(
                    &st.label,
                    size,
                    800,
                    Colors::outline(),
                    c + d * 1.5 * ctx.px,
                    Vec2::splat(0.5),
                );
            }
            ctx.text(&st.label, size, 800, Colors::cream(), c, Vec2::splat(0.5));
        }
        // Uses left: pips along the bottom of the rim.
        if st.pips_max > 0 {
            let n = st.pips_max.min(12);
            let spread = (0.36 * (n - 1) as f32).min(2.6);
            let pip = (r * 0.15).max(3.0 * ctx.px);
            for i in 0..n {
                let t = if n == 1 {
                    0.5
                } else {
                    i as f32 / (n - 1) as f32
                };
                let a = std::f32::consts::PI + spread * (t - 0.5);
                let p = c + Vec2::new(a.sin(), -a.cos()) * (r + pip * 0.7);
                let on = i < st.pips;
                let box_ = crate::node::Rect::new(p.x - pip, p.y - pip, pip * 2.0, pip * 2.0);
                ctx.rect(
                    box_,
                    if on {
                        Colors::brass_light()
                    } else {
                        Color::hexa(0x0a0806e0)
                    },
                    pip,
                    1.5 * ctx.px,
                    if on {
                        Colors::outline()
                    } else {
                        Colors::brass_dark()
                    },
                );
            }
        }
    }

    /// How urgent an alert is: its stripe, the ring round its picture and how it pulses.
    #[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
    pub enum Urgency {
        /// News: a ship launched, an island settled.
        #[default]
        Note,
        /// Wants seeing to: a shortage, unrest.
        Warning,
        /// Act now: a fire, a riot, sickness.
        Danger,
    }

    impl Urgency {
        pub fn color(self) -> Color {
            match self {
                Urgency::Note => Color::hex(0x6fb0e0),
                Urgency::Warning => Color::hex(0xf0b040),
                Urgency::Danger => Color::hex(0xf0583a),
            }
        }
    }

    /// One alert for [`alert`].
    #[derive(Clone, Debug, Default)]
    pub struct AlertSpec {
        /// The picture on its badge.
        pub icon: String,
        pub title: String,
        /// A line or two on what and where.
        pub text: String,
        /// How long ago, already in words ("just now", "2 min").
        pub age: String,
        pub urgency: Urgency,
        /// The action of its "Go there" button and of its badge ("" for none).
        pub go: String,
        /// The action of its close button ("" for none).
        pub dismiss: String,
        /// New since the player last looked: the badge's ring breathes.
        pub fresh: bool,
        /// How many of the same are folded into it (shown as "×3" when more than one).
        pub count: u32,
        /// Its width in px (0: 380).
        pub width: f32,
    }

    /// An alert strip: an urgency stripe down its left edge, the picture in a ringed badge,
    /// the title with how long ago, a line of detail, and "Go there" and close buttons. Stack
    /// a few in a column at a screen edge.
    pub fn alert(a: &AlertSpec) -> El {
        let c = a.urgency.color();
        let width = if a.width > 0.0 { a.width } else { 380.0 };
        let state = SocketState {
            ready: a.fresh,
            color: c,
            ..Default::default()
        };
        let stripe = El::new().with_style(move |s| {
            s.position(crate::style::Position::Absolute)
                .left(4.0)
                .top(6.0)
                .bottom(6.0)
                .width(4.0)
                .radius(Corners::all(2.0))
                .background(Fill::vertical(c.lerp(Color::hex(0xffffff), 0.3), c))
                .shadow(Shadow::glow(c.with_alpha(0.6), 10.0))
                .pick(Pick::Ignore)
        });
        let words = El::new()
            .class("col")
            .with_style(|s| s.gap(2.0).grow(1.0).shrink(1.0).min_width(0.0))
            .child(
                El::new()
                    .class("row")
                    .with_style(|s| s.gap(8.0).align_items(Align::Center))
                    .child(
                        El::text(a.title.clone())
                            .text_style(|t| t.size(16.0).bold().nowrap())
                            .with_style(|s| s.color(Colors::cream()))
                            .pick_ignore(),
                    )
                    .child_opt((a.count > 1).then(|| {
                        El::text(format!("×{}", a.count))
                            .text_style(|t| t.size(12.0).bold().nowrap())
                            .with_style(move |s| {
                                s.color(Colors::ink())
                                    .padding([1.0, 6.0])
                                    .radius(8.0)
                                    .background(Fill::Solid(c))
                            })
                            .pick_ignore()
                    }))
                    .child(El::new().with_style(|s| s.grow(1.0)))
                    .child_opt((!a.age.is_empty()).then(|| {
                        El::text(a.age.clone())
                            .text_style(|t| t.size(12.0).italic().nowrap())
                            .with_style(|s| s.color(Colors::cream_dim()))
                            .pick_ignore()
                    })),
            )
            .child_opt((!a.text.is_empty()).then(|| {
                El::text(a.text.clone())
                    .text_style(|t| t.size(13.5))
                    .with_style(|s| s.color(Colors::cream_dim()))
                    .pick_ignore()
            }));
        let buttons = El::new()
            .class("col")
            .with_style(|s| s.gap(6.0).align_items(Align::End).shrink(0.0))
            .child_opt((!a.dismiss.is_empty()).then(|| {
                El::new()
                    .class("button")
                    .on_click(a.dismiss.clone())
                    .with_style(|s| {
                        s.size(20.0, 20.0)
                            .padding(3.0)
                            .radius(10.0)
                            .background(Fill::Solid(Color::hexa(0x00000000)))
                            .opacity(0.7)
                    })
                    .child(
                        El::icon("ui_close").with_style(|s| s.size(14.0, 14.0).pick(Pick::Ignore)),
                    )
            }))
            .child_opt((!a.go.is_empty()).then(|| {
                El::button("Go there", a.go.clone())
                    .class("brass_button")
                    .with_style(|s| s.padding([3.0, 10.0]).font_size(13.0))
            }));
        El::new()
            .class("brass_alert")
            .with_style(move |s| s.width(width))
            .child(stripe)
            .child(socket_with(
                Some((&a.icon, Rank::Common)),
                44.0,
                &a.go,
                &state,
                "",
            ))
            .child(words)
            .child(buttons)
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

/// Smoked holographic glass: chamfered plates with scanlines and a cool glare, neon cyan
/// hairlines and glows, a squared technical face for titles and buttons, and a band of light
/// across whatever is selected. A near-future look for sci-fi games (the showcase), made by
/// restyling the built-in screens' own classes, so every screen changes at once.
pub mod glass {
    use crate::color::Color;
    use crate::style::{
        Bevel, CornerShape, Corners, Fill, Material, MaterialKind, Shadow, StylePatch,
    };
    use crate::text::TECH_FONT;
    use crate::theme::{Palette, Theme};

    fn s() -> StylePatch {
        StylePatch::default()
    }

    /// Cyan on smoked teal-black, magenta for secondary readouts.
    pub fn palette() -> Palette {
        Palette {
            panel_top: Color::hexa(0x0f2432e8),
            panel_bottom: Color::hexa(0x050d16f2),
            line: Color::hexa(0x5fe1ff40),
            surface: Color::hexa(0x0e2433dc),
            surface_hover: Color::hexa(0x173a50e8),
            well: Color::hexa(0x02070dcc),
            text: Color::hex(0xe4fbff),
            text_dim: Color::hex(0x8fb8c8),
            text_faint: Color::hex(0x56798a),
            accent: Color::hex(0x5fe1ff),
            accent_light: Color::hex(0xbff4ff),
            accent_dark: Color::hex(0x1d86a8),
            info: Color::hex(0xff5fb4),
            good: Color::hex(0x5cf2a0),
            bad: Color::hex(0xff4f6a),
            warn: Color::hex(0xffc04f),
            ..Palette::default()
        }
    }

    fn cyan(a: f32) -> Color {
        Color::hex(0x5fe1ff).with_alpha(a)
    }

    /// The glass of a big panel: chamfered, scanlined, a hairline inside the rim, a faint
    /// cyan glow and a deep shadow.
    fn plate(cut: f32, strength: f32) -> StylePatch {
        s().corner_shape(CornerShape::Chamfer)
            .radius(Corners::all(cut))
            .material(Material::new(MaterialKind::Glass).strength(strength))
            .border_color(cyan(0.5))
            .bevel(Bevel::raised(1.0, 0.18, 0.25).with_line(cyan(0.22), 1.0, cut * 0.4 + 3.0))
            .shadow(Shadow::glow(cyan(0.16), 22.0))
            .shadow2(Shadow::new(Color::hexa(0x000000a0), 40.0, 14.0))
    }

    fn small(cut: f32) -> StylePatch {
        s().corner_shape(CornerShape::Chamfer)
            .radius(Corners::all(cut))
            .material(Material::new(MaterialKind::Glass).strength(0.55).seed(3.0))
    }

    fn tech() -> StylePatch {
        s().font_family(TECH_FONT)
    }

    /// The glass theme: the built-in classes from [`palette`], then restyled.
    pub fn theme() -> Theme {
        let mut t = Theme::from_palette(palette());
        let p = t.palette.clone();
        let mut patch = |name: &str, f: &dyn Fn(&mut crate::theme::ClassStyle)| {
            if let Some(c) = t.classes.get_mut(name) {
                f(c);
            }
        };
        for name in ["panel", "glass", "dialogue_box"] {
            patch(name, &|c| c.base.merge(&plate(16.0, 1.0)));
        }
        patch("hud_panel", &|c| c.base.merge(&plate(10.0, 0.8)));
        patch("toast", &|c| c.base.merge(&plate(10.0, 0.8)));
        patch("tooltip", &|c| {
            c.base
                .merge(&plate(10.0, 0.7).merged(&s().shadow2(Shadow::new(
                    Color::hexa(0x000000c0),
                    26.0,
                    8.0,
                ))))
        });
        for name in ["card", "list_item", "menu_item", "setting_row"] {
            patch(name, &|c| {
                c.base.merge(&small(8.0));
                c.selected.merge(&s().sheen(1.0).border_color(cyan(0.9)));
            });
        }
        patch("slot", &|c| {
            c.base
                .merge(&small(9.0).merged(&s().inset_shadow(Shadow::new(
                    Color::hexa(0x000000a0),
                    12.0,
                    0.0,
                ))));
            c.selected.merge(&s().sheen(1.0));
        });
        patch("button", &|c| {
            c.base
                .merge(&small(7.0).merged(&tech().border_color(cyan(0.45))));
            c.hover.merge(&s().shadow(Shadow::glow(cyan(0.35), 14.0)));
        });
        patch("primary", &|c| {
            c.base.merge(
                &s().background(Fill::vertical(p.accent_light, p.accent))
                    .color(Color::hex(0x03141c))
                    .shadow(Shadow::glow(cyan(0.45), 18.0))
                    .sheen(0.6),
            )
        });
        patch("danger", &|c| {
            c.base.merge(&s().border_color(p.bad.with_alpha(0.8)))
        });
        patch("tab", &|c| {
            c.base.merge(&small(6.0).merged(&tech()));
            c.selected
                .merge(&s().sheen(1.0).shadow(Shadow::glow(cyan(0.3), 12.0)));
        });
        for name in ["chip", "kbd", "badge", "icon_button", "header_icon"] {
            patch(name, &|c| c.base.merge(&small(5.0)));
        }
        for name in ["bar_track", "bar_fill"] {
            patch(name, &|c| {
                c.base.merge(
                    &s().corner_shape(CornerShape::Chamfer)
                        .radius(Corners::all(3.0)),
                )
            });
        }
        for name in [
            "title",
            "heading",
            "caption",
            "label_caps",
            "stat_value",
            "slot_count",
        ] {
            patch(name, &|c| c.base.merge(&tech()));
        }
        t
    }

    /// Restyle `theme` with the glass look (its palette and the built-in classes; classes
    /// a game added itself are kept).
    pub fn install(theme: &mut Theme) {
        let g = self::theme();
        theme.palette = g.palette;
        for (k, v) in g.classes {
            theme.classes.insert(k, v);
        }
    }
}
