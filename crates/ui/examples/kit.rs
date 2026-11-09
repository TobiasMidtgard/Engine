//! The brass kit (`engine_ui::kit::brass`) on its own: a strategy-game HUD built from it over a
//! picture, and a sheet of its parts. Quick to render (no 3D scene).
//!
//! `cargo run -p engine_ui --example kit -- <out_dir> [width] [height] [backdrop.png]`
//!
//! Writes `kit_brass.png` (the HUD, a tile hovered with its tooltip) and `kit_brass_parts.png`.
//! Needs a GPU or a software Vulkan driver (lavapipe).

use engine_core::math::Vec2;
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_input::InputEvent;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, capture_now};
use engine_ui::kit::brass::{self, Colors};
use engine_ui::node::Screen;
use engine_ui::prelude::*;
use engine_ui::style::{Align, Direction, Justify, Pick, Position};

fn screen(key: &str, layer: i32) -> Screen {
    Screen {
        key: key.into(),
        modal: false,
        layer,
        closable: false,
        ..Default::default()
    }
}

fn backdrop(picture: bool) -> El {
    let root = El::new().with_style(|s| {
        s.fill_parent()
            .pick(Pick::Ignore)
            .background(Fill::vertical(Color::hex(0x5e86a0), Color::hex(0x24404f)))
    });
    if picture {
        root.child(El::icon("kit_backdrop").icon_cover(true).with_style(|s| {
            s.fill_parent()
                .width(engine_ui::style::pct(100.0))
                .height(engine_ui::style::pct(100.0))
                .pick(Pick::Ignore)
        }))
    } else {
        root
    }
}

fn top_bar() -> El {
    let stat = |icon: &str, value: &str, label: &str| {
        El::new()
            .class("row")
            .with_style(|s| s.gap(10.0))
            .child(brass::badge(icon, 46.0))
            .child(
                El::new()
                    .class("col")
                    .with_style(|s| s.gap(0.0))
                    .child(El::text(value).class("brass_number").pick_ignore())
                    .child(brass::label(label, false)),
            )
    };
    let goods = El::new().class("row").with_style(|s| s.gap(6.0)).children(
        [
            ("fish", "42"),
            ("wood_log", "120"),
            ("hammer", "30"),
            ("herb", "43"),
            ("cloak", "18"),
            ("bread", "25"),
            ("potion_stamina", "9"),
        ]
        .into_iter()
        .map(|(i, n)| {
            brass::well(i, n).tooltip("Goods", "In the warehouse of the island in view.")
        }),
    );
    let speed = El::new().class("row").with_style(|s| s.gap(8.0)).children(
        ["ui_minus", "ui_arrow_right", "ui_plus"]
            .into_iter()
            .enumerate()
            .map(|(i, icon)| brass::medallion(icon, 40.0, "speed").selected(i == 1)),
    );
    let bar = El::new()
        .class("brass_bar")
        .with_style(|s| {
            s.position(Position::Absolute)
                .left(14.0)
                .right(14.0)
                .top(10.0)
                .height(76.0)
                .padding([0.0, 22.0])
                .gap(24.0)
                .align_items(Align::Center)
        })
        .child(stat("coin", "16,343", "+291 a minute"))
        .child(stat("marker_city", "1,210", "people"))
        .child(goods)
        .child(El::new().with_style(|s| s.grow(1.0)))
        .child(
            El::new()
                .class("col")
                .with_style(|s| s.gap(0.0).align_items(Align::End))
                .child(
                    El::text("March 1821")
                        .class("brass_title")
                        .with_style(|s| s.font_size(19.0))
                        .pick_ignore(),
                )
                .child(brass::label("spring", false)),
        )
        .child(speed);
    let cartouche = El::new()
        .class("cartouche")
        .with_style(|s| {
            s.position(Position::Absolute)
                .top(66.0)
                .left(engine_ui::style::pct(50.0))
                .translate([-110.0, 0.0])
                .width(220.0)
        })
        .child(El::text("Haverholm").class("ink_title").pick_ignore())
        .child(brass::label("141 fisherfolk · 376 labourers", true));
    El::new()
        .with_style(|s| s.fill_parent().pick(Pick::Ignore))
        .child(bar)
        .child(cartouche)
}

fn tile(icon: &str, name: &str, cost: &str, selected: bool, i: usize) -> El {
    El::new()
        .class("brass_tile")
        .id(format!("tile:{i}"))
        .on_click("build")
        .selected(selected)
        .tooltip(
            name,
            format!(
                "Turns wool into clothes.\n[b]Cost[/b] {cost} coins, 4 planks. Needs 30 fisherfolk."
            ),
        )
        .with_style(|s| {
            s.direction(Direction::Column)
                .align_items(Align::Center)
                .gap(2.0)
                .padding([10.0, 6.0, 8.0, 6.0])
                .width(98.0)
                .enter(
                    engine_ui::style::Enter::rise(0.35, 24.0)
                        .delay(0.04 * i as f32)
                        .ease(engine_ui::style::Ease::Back),
                )
        })
        .child(brass::badge(icon, 50.0))
        .child(
            El::text(name)
                .class("ink")
                .text_style(|t| t.bold().size(14.0).center().nowrap())
                .pick_ignore(),
        )
        .child(
            El::new()
                .class("row")
                .with_style(|s| s.gap(3.0).justify(Justify::Center))
                .child(El::icon("coin").with_style(|s| s.size(14.0, 14.0).pick(Pick::Ignore)))
                .child(
                    El::text(cost)
                        .text_style(|t| t.size(13.0).nowrap().tabular())
                        .with_style(|s| s.color(Colors::ink_dim()))
                        .pick_ignore(),
                ),
        )
}

fn build_bar() -> El {
    let tabs = El::new().class("row").with_style(|s| s.gap(14.0)).children(
        [
            "ui_layers",
            "marker_city",
            "hammer",
            "herb",
            "star",
            "marker_spaceport",
        ]
        .into_iter()
        .enumerate()
        .map(|(i, icon)| brass::medallion(icon, 50.0, "cat").selected(i == 2)),
    );
    let tiles = El::new().class("row").with_style(|s| s.gap(12.0)).children(
        [
            ("wood_log", "Woodcutter", "100"),
            ("hammer", "Sawmill", "250"),
            ("cloak", "Weaver", "400"),
            ("fish", "Fishery", "150"),
            ("bread", "Bakery", "600"),
            ("potion_stamina", "Brewery", "700"),
        ]
        .into_iter()
        .enumerate()
        .map(|(i, (icon, name, cost))| tile(icon, name, cost, i == 2, i)),
    );
    let plate = brass::plate()
        .with_style(|s| {
            s.direction(Direction::Column)
                .align_items(Align::Center)
                .gap(12.0)
        })
        .child(
            El::new()
                .class("row")
                .with_style(|s| s.gap(14.0))
                .child(brass::divider(110.0))
                .child(El::text("Workshops").class("brass_title").pick_ignore())
                .child(brass::divider(110.0)),
        )
        .child(tabs)
        .child(tiles);
    El::new()
        .with_style(|s| s.fill_parent().pick(Pick::Ignore))
        .child(
            El::new()
                .with_style(|s| {
                    s.position(Position::Absolute)
                        .left(0.0)
                        .right(0.0)
                        .bottom(24.0)
                        .direction(Direction::Column)
                        .align_items(Align::Center)
                        .pick(Pick::Ignore)
                })
                .child(plate),
        )
}

fn card() -> El {
    let line = |k: &str, v: &str| {
        El::new()
            .class("row")
            .with_style(|s| s.justify(Justify::SpaceBetween))
            .child(El::text(k).class("ink").pick_ignore())
            .child(
                El::text(v)
                    .class("ink")
                    .text_style(|t| t.bold().tabular())
                    .pick_ignore(),
            )
    };
    let c = brass::parchment()
        .with_style(|s| {
            s.position(Position::Absolute)
                .right(22.0)
                .top(130.0)
                .width(340.0)
                .padding([18.0, 20.0])
                .gap(8.0)
                .direction(Direction::Column)
                .enter(engine_ui::style::Enter::slide(0.4, 60.0))
        })
        .child(
            El::new()
                .class("row")
                .with_style(|s| s.gap(12.0))
                .child(brass::badge("cloak", 58.0))
                .child(
                    El::new()
                        .class("col")
                        .with_style(|s| s.gap(0.0).grow(1.0))
                        .child(El::text("Weaver").class("ink_title").pick_ignore())
                        .child(brass::label("Haverholm · Workshops", true)),
                ),
        )
        .child(brass::divider(300.0))
        .child(
            El::new()
                .class("row")
                .with_style(|s| s.gap(10.0).justify(Justify::Center))
                .child(brass::badge("herb", 40.0))
                .child(
                    El::icon("ui_arrow_right")
                        .with_style(|s| s.size(22.0, 22.0).color(Colors::ink_dim())),
                )
                .child(brass::badge("cloak", 48.0)),
        )
        .child(brass::meter(300.0, 0.62))
        .child(line("Status", "Working"))
        .child(line("Output", "100%"))
        .child(line("Workers", "30 fisherfolk"))
        .child(line("Made so far", "48"))
        .child(line("Upkeep", "20 coins/min"))
        .child(
            El::new()
                .class("row")
                .with_style(|s| s.gap(10.0).margin([8.0, 0.0, 0.0, 0.0]))
                .child(El::button("Pause", "pause").class("brass_button"))
                .child(El::button("Demolish", "demolish").class("brass_button_dark")),
        );
    El::new()
        .with_style(|s| s.fill_parent().pick(Pick::Ignore))
        .child(c)
}

fn main() {
    engine_core::log::init("warn");
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dir = std::path::PathBuf::from(
        args.first()
            .cloned()
            .unwrap_or_else(|| "screenshots/ui".into()),
    );
    let w: u32 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(1600);
    let h: u32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(900);
    let picture = args.get(3).cloned();
    std::fs::create_dir_all(&dir).expect("output dir");

    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut win = app.world.resource_mut::<Window>();
        win.width = w;
        win.height = h;
    }
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 60.0,
        });
    app.add_plugins((RenderPlugin, engine_input::InputPlugin, UiPlugin));
    app.update_n(2);
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("no GPU: {}", d.0);
        return;
    }
    brass::install(&mut app.world.resource_mut::<Theme>());
    let has_picture = match picture.as_deref().map(image::open) {
        Some(Ok(img)) => {
            app.world
                .resource_mut::<engine_ui::IconLibrary>()
                .add_picture("kit_backdrop", img.to_rgba8());
            true
        }
        Some(Err(e)) => {
            eprintln!("backdrop: {e}");
            false
        }
        None => false,
    };
    let spawn = |app: &mut App, key: &str, layer: i32, el: &El| {
        if let Err(e) = engine_ui::screens::spawn_screen(&mut app.world, screen(key, layer), el) {
            eprintln!("{key}: {e}");
        }
    };
    spawn(&mut app, "kit_back", -10, &backdrop(has_picture));
    spawn(&mut app, "kit_top", 0, &top_bar());
    spawn(&mut app, "kit_bar", 0, &build_bar());
    spawn(&mut app, "kit_card", 1, &card());
    app.update_n(60);
    // Hover the third tile for its lift and tooltip.
    let e = engine_ui::input::find_id(&mut app.world, "tile:3");
    if let Some(p) = e
        .and_then(|e| app.world.get::<ComputedNode>(e))
        .map(|c| c.rect.center())
    {
        app.world.write_message(InputEvent::CursorMoved {
            position: p + Vec2::new(10.0, 6.0),
        });
    }
    app.update_n(70);
    let path = dir.join("kit_brass.png");
    match capture_now(&mut app.world, &path) {
        Ok(_) => println!("wrote {}", path.display()),
        Err(e) => eprintln!("kit_brass: {e}"),
    }
}
