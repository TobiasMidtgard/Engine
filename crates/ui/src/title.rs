//! The title screen: the game's name over a slowly turning view of the home world, the
//! galaxy and a moon, with Continue (the newest save), New Game, Load Game, Settings and
//! Quit, and a card for the save Continue picks up.
//!
//! ```ignore
//! ui::show(world, "title", &json!({}))?;
//! ```
//!
//! New Game opens the loading screen for Veyra. Menu items send `game.new`, `game.load:<id>` (through [`crate::saves::load`]) and, after a
//! confirm, `game.exit` for the game; Load Game and Settings open those screens over it.

use bevy_ecs::message::MessageCursor;
use bevy_ecs::prelude::*;
use engine_core::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::color::Color;
use crate::draw::{DrawCtx, Painters};
use crate::el::El;
use crate::input::UiEvent;
use crate::map::PlanetKind;
use crate::menus::menu_item;
use crate::node::*;
use crate::saves::{self, SaveSlots, playtime};
use crate::screens;
use crate::style::{Align, Corners, Justify, Pick, Position};
use crate::theme::Theme;
use crate::toast::{self, ToastKind};
use crate::widgets;

/// The backdrop's clock (on the node with `paint: "title_backdrop"`).
#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TitleView {
    /// Real seconds since the screen opened (the world turns, the stars drift).
    pub time: f32,
}

fn paint_backdrop(world: &mut World, e: Entity, ctx: &mut DrawCtx) {
    let t = world.get::<TitleView>(e).map_or(0.0, |v| v.time);
    let r = ctx.rect;
    let (w, h) = (r.w, r.h);
    let at = |x: f32, y: f32| Vec2::new(r.x + w * x, r.y + h * y);
    // Deep space, a touch of blue toward the planet.
    ctx.rect_fill(
        r,
        crate::style::Fill::Linear {
            from: Color::hex(0x02030a),
            to: Color::hex(0x07101f),
            angle_deg: 20.0,
        },
        [0.0; 4],
        0.0,
        Color::TRANSPARENT,
    );
    ctx.starfield(r, Vec2::new(t * 4.0, t * 1.2), 1.0, 7.3, 0.0);
    // Nebula light behind the galaxy.
    ctx.glow(
        at(0.70, 0.22),
        h * 0.55,
        Color::hex(0x6a3aa8).with_alpha(0.10),
        1.2,
    );
    ctx.glow(
        at(0.86, 0.12),
        h * 0.38,
        Color::hex(0x2a8ab8).with_alpha(0.08),
        1.2,
    );
    ctx.glow(
        at(0.40, 0.05),
        h * 0.40,
        Color::hex(0xa83a6a).with_alpha(0.05),
        1.2,
    );
    ctx.galaxy(r, at(0.74, 0.24), h * 0.30, 0.6 + t * 0.006, 0.34);
    // The sun just off the top left, and its glare.
    let sun = at(0.06, -0.04);
    ctx.glow(sun, h * 0.9, Color::hex(0xffd8a0).with_alpha(0.10), 1.1);
    ctx.glow(sun, h * 0.35, Color::hex(0xfff2d8).with_alpha(0.28), 1.4);
    // Veyra filling the lower right, turning slowly, lit from the sun's side.
    let pc = at(0.76, 1.18);
    let pr = h * 0.78;
    ctx.glow(pc, pr * 1.18, Color::hex(0x4aa0ff).with_alpha(0.12), 1.4);
    ctx.planet(
        pc,
        pr,
        PlanetKind::Terran.look(3.1),
        0.4 + t * 0.012,
        0.18,
        -2.3,
        0.55,
        0.06,
    );
    // A small moon over the planet's limb, lit from the sun's side (in front of the
    // backlit world, so it reads as a crescent-to-half disc rather than a black hole).
    let moon = at(0.90, 0.40);
    ctx.glow(moon, h * 0.11, Color::hex(0xc8d4e8).with_alpha(0.07), 1.4);
    ctx.planet(
        moon,
        h * 0.045,
        PlanetKind::Ice.look(8.4),
        0.4 + t * 0.012,
        0.1,
        -1.15,
        0.35,
        0.0,
    );
    // Darker on the left, where the menu sits.
    ctx.rect_fill(
        r,
        crate::style::Fill::Linear {
            from: Color::hexa(0x02040ad8),
            to: Color::hexa(0x02040a00),
            angle_deg: 0.0,
        },
        [0.0; 4],
        0.0,
        Color::TRANSPARENT,
    );
    ctx.rect_fill(
        r,
        crate::style::Fill::Radial {
            inner: Color::hexa(0x00000000),
            outer: Color::hexa(0x00000090),
        },
        [0.0; 4],
        0.0,
        Color::TRANSPARENT,
    );
}

pub fn add_painters(p: &mut Painters) {
    p.add("title_backdrop", paint_backdrop);
}

/// The open title screen (on its root).
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct TitleScreen;

fn continue_card(world: &World) -> Option<El> {
    let theme = world.resource::<Theme>();
    let p = &theme.palette;
    let slots = world.get_resource::<SaveSlots>()?;
    let s = slots.slots.first()?;
    let title = s.title(0);
    Some(
        El::new()
            .class("hud_panel row")
            .id("title_continue_card")
            .on_click("title.continue")
            .with_style(|x| {
                x.gap(16.0)
                    .padding([12.0, 18.0, 12.0, 12.0])
                    .radius(Corners::all(14.0))
                    .width(480.0)
                    .align_items(Align::Center)
            })
            .child(
                El::new()
                    .paint("save_thumb")
                    .attach(
                        "SaveThumb",
                        &saves::SaveThumb {
                            scene: s.scene.clone(),
                            seed: s.seed,
                        },
                    )
                    .with_style(|x| {
                        x.size(160.0, 90.0)
                            .shrink(0.0)
                            .radius(Corners::all(6.0))
                            .border(1.0, Color::WHITE.with_alpha(0.16))
                            .pick(Pick::Ignore)
                    }),
            )
            .child(
                El::new()
                    .class("col")
                    .with_style(|x| x.gap(4.0).grow(1.0).shrink(1.0))
                    .child(
                        El::text(format!("CONTINUE  ·  {}", title.to_uppercase()))
                            .text_style(|t| t.bold().nowrap())
                            .with_style(|x| x.font_size(12.0).color(p.accent))
                            .pick_ignore(),
                    )
                    .child(
                        El::text(s.chapter.clone())
                            .text_style(|t| t.semibold().nowrap())
                            .with_style(|x| x.font_size(17.0).color(Color::WHITE))
                            .pick_ignore(),
                    )
                    .child(
                        El::text(format!("{}  ·  Level {}", s.location, s.level))
                            .text_style(|t| t.nowrap())
                            .with_style(|x| x.font_size(13.0).color(p.text_dim))
                            .pick_ignore(),
                    )
                    .child(
                        El::text(format!(
                            "{}  ·  {} played",
                            s.saved_at,
                            playtime(s.playtime_s)
                        ))
                        .text_style(|t| t.nowrap())
                        .with_style(|x| x.font_size(12.0).color(p.text_faint))
                        .pick_ignore(),
                    ),
            ),
    )
}

fn build(world: &World) -> El {
    let theme = world.resource::<Theme>();
    let p = &theme.palette;
    let has_save = world
        .get_resource::<SaveSlots>()
        .is_some_and(|s| !s.slots.is_empty());
    let mut menu = El::new()
        .class("col")
        .id("title_menu")
        .with_style(|s| s.gap(4.0).width(420.0));
    if has_save {
        menu = menu.child(menu_item(
            "ui_arrow_right",
            "Continue",
            "title.continue",
            "",
            "title_continue",
        ));
    }
    menu = menu
        .child(menu_item("star", "New Game", "title.new", "", "title_new"))
        .child(menu_item(
            "map_scroll",
            "Load Game",
            "title.load",
            "",
            "title_load",
        ))
        .child(menu_item(
            "ui_settings",
            "Settings",
            "title.settings",
            "",
            "title_settings",
        ))
        .child(
            El::new()
                .class("divider")
                .with_style(|s| s.margin([8.0, 0.0])),
        )
        .child(menu_item(
            "ui_close",
            "Quit",
            "title.quit",
            "",
            "title_quit",
        ));
    let logo = El::new()
        .class("col")
        .with_style(|s| s.gap(8.0))
        .child(
            El::text("A  STORY  OF  THE  VEIL  GALAXY")
                .text_style(|t| t.bold().nowrap())
                .with_style(|s| s.font_size(13.0).color(p.accent)),
        )
        .child(
            El::text("THE VEIL")
                .text_style(|t| t.bold().nowrap().shadow())
                .with_style(|s| s.font_size(104.0).color(Color::WHITE)),
        )
        .child(
            El::new()
                .with_style(|s| {
                    s.size(180.0, 3.0)
                        .radius(Corners::all(2.0))
                        .bg(p.accent)
                        .shadow(crate::style::Shadow::glow(p.accent.fade(0.6), 12.0))
                })
                .pick_ignore(),
        )
        .child(
            El::text("Twelve worlds, one silent relay, and the long way home.")
                .text_style(|t| t.nowrap().shadow())
                .with_style(|s| {
                    s.font_size(18.0)
                        .font_weight(600)
                        .color(p.text_dim)
                        .margin([6.0, 0.0, 0.0, 0.0])
                }),
        );
    let left = El::new()
        .class("col")
        .with_style(|s| {
            s.position(Position::Absolute)
                .left(140.0)
                .top(0.0)
                .bottom(0.0)
                .justify(Justify::Center)
                .gap(40.0)
        })
        .child(logo)
        .child(menu)
        .child_opt(continue_card(world));
    let foot = El::new()
        .class("row")
        .with_style(|s| {
            s.position(Position::Absolute)
                .left(140.0)
                .right(80.0)
                .bottom(44.0)
                .gap(22.0)
                .align_items(Align::Center)
        })
        .child(
            El::text("Engine UI showcase  ·  build 0.1  ·  Rust + wgpu")
                .class("caption")
                .text_style(|t| t.nowrap()),
        )
        .child(El::new().class("spacer"))
        .child(widgets::key_hint("Enter", "A", "Select"))
        .child(widgets::key_hint("↑ ↓", "", "Move"));
    El::new()
        .class("screen")
        .child(
            El::new()
                .id("title_backdrop")
                .paint("title_backdrop")
                .attach("TitleView", &TitleView::default())
                .with_style(|s| s.fill_parent().pick(Pick::Block)),
        )
        .child(left)
        .child(foot)
}

/// Builder for `ui.show {"screen": "title"}`.
pub fn build_screen(world: &mut World, _args: &Value) -> Result<Entity, String> {
    let el = build(world);
    let sp = screens::spawn_screen(
        world,
        Screen {
            modal: true,
            layer: 18,
            closable: false,
            ..Default::default()
        },
        &el,
    )?;
    let root = sp.root.ok_or("empty")?;
    world.entity_mut(root).insert(TitleScreen);
    Ok(root)
}

/// Title menu actions and the backdrop's clock.
pub fn title_system(world: &mut World, mut seen: Local<MessageCursor<UiEvent>>) {
    let events: Vec<UiEvent> = crate::input::read_events(world, &mut seen);
    let mut q = world.query_filtered::<(Entity, &Screen), With<TitleScreen>>();
    let Some(root) = q.iter(world).find(|(_, s)| !s.closing).map(|(e, _)| e) else {
        return;
    };
    for ev in &events {
        let UiEvent::Action { action, entity, .. } = ev else {
            continue;
        };
        if entity.is_some_and(|e| !screens::is_descendant(world, e, root)) {
            continue;
        }
        match action.as_str() {
            "title.continue" => {
                let first = world
                    .get_resource::<SaveSlots>()
                    .and_then(|s| s.slots.first())
                    .map(|s| s.id.clone());
                if let Some(id) = first
                    && let Err(e) = saves::load(world, &id)
                {
                    toast::notify(world, ToastKind::Bad, "", "", &e, 2.4);
                }
            }
            "title.new" => {
                crate::input::dispatch(world, "game.new");
                screens::close_screen(world, root, false);
                let args = json!({"planet": "veyra", "place": "Landfall"});
                if let Err(e) = screens::show(world, "loading", &args) {
                    toast::notify(world, ToastKind::Bad, "", "", &e, 2.4);
                }
            }
            "title.load" => {
                if let Err(e) = screens::show(world, "saves", &json!({"mode": "load"})) {
                    toast::notify(world, ToastKind::Bad, "", "", &e, 2.4);
                }
            }
            "title.settings" => {
                if let Err(e) = screens::show(world, "settings", &Value::Null) {
                    toast::notify(world, ToastKind::Bad, "", "", &e, 2.4);
                }
            }
            "title.quit" => {
                let args = json!({"title": "Quit to desktop?", "text": "Your last save is kept.", "confirm": "Quit", "action": "game.exit", "danger": true});
                if let Err(e) = screens::show(world, "confirm", &args) {
                    toast::notify(world, ToastKind::Bad, "", "", &e, 2.4);
                }
            }
            _ => {}
        }
    }
    let dt = world.resource::<Time>().real_delta_secs().min(0.1);
    if let Some(bg) = screens::find_in(world, root, "title_backdrop")
        && let Some(mut v) = world.get_mut::<TitleView>(bg)
    {
        v.time += dt;
    }
}
