//! The front of the game: its name, the title screen over the live world, the Esc menu and the
//! how-to-play page (F1).
//!
//! The title opens over a new game that is already set up: the economy stands still
//! (`Economy::speed` 0), the bars are hidden and the camera drifts beside the company's barque.
//! New game gives the bars and the clock back; Sandbox town starts the game again with
//! `--sandbox` (the finished town of earlier rounds); How to play opens the page; Quit asks
//! first. In play, Esc opens the menu when it has nothing else to do (no tool in hand, nothing
//! picked, no book open), and F1 opens the page.
//!
//! The title is left out with `--sandbox`, `--tour`, `--play`, `--headless` and `--no-title`.
//! A played game (not headless, not scripted) runs borderless fullscreen; F11 or `--windowed`
//! gives a window.
//!
//! ```sh
//! cargo run -p archipelago --release -- --headless --size 2560x1440 --title-shots
//! ```
//! writes `screenshots/archipelago/title/{title,help,start,menu}.png`.

use bevy_ecs::message::MessageCursor;
use engine::prelude::*;
use engine::render::{RenderDisabled, capture_now};
use engine_ui::color::Color;
use engine_ui::el::El;
use engine_ui::input::UiEvent;
use engine_ui::kit::brass::{self, Colors};
use engine_ui::node::{Node, Screen};
use engine_ui::screens;
use engine_ui::style::{
    Align, Corners, Direction, Ease, Enter, Fill, Justify, Pick, Position, Shadow, StylePatch, pct,
};
use engine_ui::text::{DISPLAY_CAPS_FONT, DISPLAY_FONT, HUMANIST_FONT};
use engine_ui::theme::ClassStyle;
use engine_ui::toast::{self, ToastKind};

use crate::camera::StrategyCam;
use crate::sim::Economy;
use crate::tools::{Hand, Tool};

/// The game's name: the window, the title screen, the menus and the docs.
pub const NAME: &str = "Brightsund";
/// The line under the name on the title screen.
pub const TAGLINE: &str = "One ship, a sea of fog, and a company to build.";

const TITLE: &str = "fv_title";
const MENU: &str = "fv_menu";
const HELP: &str = "fv_help";
const CONFIRM: &str = "fv_confirm";
/// The HUD's books and the selected building's card: while one is open, Esc is theirs.
const ESC_TAKERS: [&str; 5] = [
    "isle_info",
    "isle_ledger",
    "isle_routes",
    "isle_fleet",
    "isle_shelf",
];

/// What the front of the game shows.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Stage {
    /// The game, with its bars.
    #[default]
    Playing,
    /// The title screen over the new game.
    Title,
}

/// The front's state between frames.
#[derive(Resource, Default)]
pub struct Front {
    pub stage: Stage,
    /// The title screen has been put up.
    opened: bool,
    seen: MessageCursor<UiEvent>,
    /// Real seconds the title has been up (the camera's drift).
    t: f32,
    /// The view the title drifts round: focus, yaw, distance.
    title_view: Option<(DVec3, f32, f32)>,
    /// The view the game starts with, put back on New game.
    start_view: Option<(DVec3, f32, f32)>,
    /// The game speed to put back when the Esc menu closes.
    resume_speed: Option<f32>,
    /// Last frame, nothing but the map wanted Esc.
    esc_free: bool,
}

/// Add the front to the game: the title (unless the run is a tour, a playtest, headless or a
/// sandbox), the menu and page keys, and fullscreen for a played game.
pub fn add(app: &mut App) {
    let args: Vec<String> = std::env::args().collect();
    let has = |f: &str| args.iter().any(|a| a == f);
    let headless = app.world.resource::<EngineConfig>().headless;
    let scripted = ["--tour", "--play", "--title-shots"].iter().any(|f| has(f));
    let title = !headless && !scripted && !has("--sandbox") && !has("--no-title");
    app.insert_resource(Front {
        stage: if title { Stage::Title } else { Stage::Playing },
        ..Default::default()
    });
    if !headless && !scripted && !has("--windowed") {
        app.world.resource_mut::<Window>().fullscreen = true;
    }
    app.add_systems(Update, front);
}

/// Menu actions, keys, the title's drift and Esc's state.
fn front(world: &mut World) {
    if !world.contains_resource::<Front>() || !world.contains_resource::<Hand>() {
        return;
    }
    if world.resource::<Front>().stage == Stage::Title && !world.resource::<Front>().opened {
        open_title(world);
    }
    let mut seen = std::mem::take(&mut world.resource_mut::<Front>().seen);
    let events = engine_ui::input::read_events(world, &mut seen);
    world.resource_mut::<Front>().seen = seen;
    for ev in events {
        if let UiEvent::Action { action, .. } = ev {
            act(world, &action);
        }
    }
    keys(world);
    if world.resource::<Front>().stage == Stage::Title {
        drift(world);
        show_bars(world, false);
    }
    // What Esc would do next frame: the map's tools and the HUD's books come first.
    let free = *world.resource::<Tool>() == Tool::Select
        && world.resource::<Hand>().selected.is_none()
        && !ESC_TAKERS
            .iter()
            .any(|k| screens::find_screen(world, k).is_some());
    world.resource_mut::<Front>().esc_free = free;
}

fn act(world: &mut World, a: &str) {
    match a {
        "fv.new" => start(world),
        "fv.sandbox" => relaunch(world, &["--sandbox"]),
        "fv.help" => open_help(world),
        "fv.help.close" => {
            screens::hide(world, HELP, false);
        }
        "fv.resume" => close_menu(world),
        "fv.restart" => confirm(
            world,
            "Start a new game?",
            "The game starts again with one ship off Haverholm. This game is not saved.",
            "New game",
            "fv.restart.now",
        ),
        "fv.restart.now" => relaunch(world, &["--no-title"]),
        "fv.quit" => confirm(
            world,
            &format!("Quit {NAME}?"),
            "The game is not saved: the company and its islands are gone when you quit.",
            "Quit",
            "fv.exit",
        ),
        "fv.exit" => {
            world.write_message(AppExit::Success);
        }
        "fv.confirm.cancel" => {
            screens::hide(world, CONFIRM, false);
        }
        _ => {
            if let Some(then) = a.strip_prefix("fv.confirm.ok:") {
                screens::hide(world, CONFIRM, true);
                act(world, then);
            }
        }
    }
}

fn keys(world: &mut World) {
    let Some(keys) = world.get_resource::<ButtonInput<KeyCode>>() else {
        return;
    };
    let (esc, f1, f11) = (
        keys.just_pressed(KeyCode::Escape),
        keys.just_pressed(KeyCode::F1),
        keys.just_pressed(KeyCode::F11),
    );
    if f11 && !world.resource::<EngineConfig>().headless {
        let mut w = world.resource_mut::<Window>();
        w.fullscreen = !w.fullscreen;
    }
    let (esc_free, stage) = {
        let f = world.resource::<Front>();
        (f.esc_free, f.stage)
    };
    if screens::find_screen(world, CONFIRM).is_some() {
        if esc {
            screens::hide(world, CONFIRM, false);
        }
        return;
    }
    let help = screens::find_screen(world, HELP).is_some();
    if f1 {
        if help {
            screens::hide(world, HELP, false);
        } else {
            open_help(world);
        }
        return;
    }
    if !esc {
        return;
    }
    if help {
        screens::hide(world, HELP, false);
    } else if screens::find_screen(world, MENU).is_some() {
        close_menu(world);
    } else if stage == Stage::Playing && esc_free {
        open_menu(world);
    }
}

// ------------------------------------------------------------------------------ the title

/// Put up the title over the new game: the bars hidden, the clock stopped, the view beside
/// the barque.
fn open_title(world: &mut World) {
    world.resource_mut::<Front>().opened = true;
    install_classes(world);
    world.resource_mut::<Economy>().speed = 0.0;
    let start = {
        let mut q = world.query::<&StrategyCam>();
        q.iter(world)
            .next()
            .map(|c| (c.goal_focus, c.goal_yaw, c.goal_dist))
    };
    let view = title_view(world).or(start);
    {
        let mut f = world.resource_mut::<Front>();
        f.start_view = start;
        f.title_view = view;
        f.t = 0.0;
    }
    if let Some((focus, yaw, dist)) = view {
        let mut q = world.query::<&mut StrategyCam>();
        if let Some(mut cam) = q.iter_mut(world).next() {
            cam.goal_focus = focus;
            cam.goal_yaw = yaw;
            cam.goal_dist = dist;
            cam.pitch_override = Some(13f32.to_radians());
            cam.snap();
        }
    }
    show_bars(world, false);
    // The fog of war stands as a grey wall on the horizon from this low view.
    if let Some(mut fog) = world.get_resource_mut::<crate::fog::FogOfWar>() {
        fog.shown = false;
    }
    let screen = Screen {
        key: TITLE.into(),
        modal: true,
        layer: 30,
        closable: false,
        ..Default::default()
    };
    if let Err(e) = screens::spawn_screen(world, screen, &title_el()) {
        engine::core::log::warn!("{NAME} title: {e}");
    }
}

/// From the sea, looking past the company's barque at the coast it lies off, with the ship
/// right of the middle (the menu is on the left).
fn title_view(world: &mut World) -> Option<(DVec3, f32, f32)> {
    let at = {
        let mut q = world.query::<&crate::fleet::Vessel>();
        q.iter(world)
            .find(|v| v.name == crate::settle::FIRST_SHIP)
            .map(|v| v.at)?
    };
    let coast = world.get_resource::<crate::town::TownCentre>()?.0;
    let d = DVec2::new(coast.x - at.x, coast.z - at.z).normalize_or_zero();
    if d == DVec2::ZERO {
        return None;
    }
    // The camera looks along (-sin yaw, -cos yaw) on the map.
    let yaw = (-d.x).atan2(-d.y) as f32;
    let dist = 230.0;
    let right = DVec2::new(yaw.cos() as f64, -(yaw.sin() as f64));
    let f = DVec2::new(at.x, at.z) + d * 60.0 - right * (dist as f64 * 0.32);
    Some((DVec3::new(f.x, 0.0, f.y), yaw - 0.18, dist))
}

/// A new game's first view: from the sea over the barque to the coast. The barque sits just
/// above the build bar, which covers the lower third of the screen in the middle, and the
/// coast near the top.
fn first_view(world: &mut World) -> Option<(DVec3, f32, f32)> {
    let at = {
        let mut q = world.query::<&crate::fleet::Vessel>();
        q.iter(world)
            .find(|v| v.name == crate::settle::FIRST_SHIP)
            .map(|v| v.at)?
    };
    let coast = world.get_resource::<crate::town::TownCentre>()?.0;
    let d = DVec2::new(coast.x - at.x, coast.z - at.z);
    let len = d.length();
    if len < 1.0 {
        return None;
    }
    let fov = {
        let mut q = world.query::<&Camera>();
        match q.iter(world).next().map(|c| c.projection) {
            Some(engine::render::Projection::Perspective { fov_y_deg, .. }) => {
                fov_y_deg.to_radians()
            }
            _ => 50f32.to_radians(),
        }
    };
    // Turned off the line, the view's depth along it is shorter.
    let (f, dist) = frame_two(len * (FIRST_TURN as f64).cos(), fov, BARQUE_Y, COAST_Y);
    engine::core::log::info!("{NAME}: the first view, {len:.0} m to the coast, from {dist:.0} m");
    let yaw = (-d.x).atan2(-d.y) as f32 + FIRST_TURN;
    let mid = DVec2::new(at.x, at.z) + d * f;
    Some((DVec3::new(mid.x, 0.0, mid.y), yaw, dist))
}

/// How far the first view turns left off the line from the barque to the coast, radians: she
/// lies bows to the shore, and seen from dead astern she is only a stack of sails.
const FIRST_TURN: f32 = 0.5;

/// Where the first view puts the barque and the coast on screen, from -1 (bottom) to 1 (top):
/// as far apart as the bars allow, since a wider view shows the fog of war's edges.
const BARQUE_Y: f64 = -0.17;
const COAST_Y: f64 = 0.7;

/// Where a ground point `s` metres ahead of the focus shows on screen, from -1 (bottom) to 1
/// (top), for a view `dist` away at the zoom's own pitch.
fn screen_y(s: f64, dist: f32, fov_y: f32) -> f64 {
    let p = StrategyCam::pitch_for(dist) as f64;
    let dist = dist as f64;
    let below = (dist * p.sin()).atan2(dist * p.cos() + s);
    (p - below).tan() / (fov_y as f64 * 0.5).tan()
}

/// The focus (as a share of the way from the barque to the coast, `len` metres) and the
/// distance that put the barque at `barque_y` and the coast at `coast_y` on screen.
fn frame_two(len: f64, fov_y: f32, barque_y: f64, coast_y: f64) -> (f64, f32) {
    let mut best = (f64::MAX, 0.45, 560.0);
    for i in 0..=50 {
        let f = i as f64 * 0.01;
        for j in 0..=80 {
            let dist = 300.0 + j as f32 * 20.0;
            let e = (screen_y(-f * len, dist, fov_y) - barque_y).powi(2)
                + (screen_y((1.0 - f) * len, dist, fov_y) - coast_y).powi(2);
            if e < best.0 {
                best = (e, f, dist);
            }
        }
    }
    (best.1, best.2)
}

/// The title's slow sway round its view.
fn drift(world: &mut World) {
    let dt = world.resource::<Time>().real_delta_secs().min(0.1);
    let (t, view) = {
        let mut f = world.resource_mut::<Front>();
        f.t += dt;
        (f.t, f.title_view)
    };
    let Some((focus, yaw, dist)) = view else {
        return;
    };
    let mut q = world.query::<&mut StrategyCam>();
    if let Some(mut cam) = q.iter_mut(world).next() {
        cam.goal_focus = focus;
        cam.goal_yaw = yaw + 0.10 * (t * 0.045).sin();
        cam.goal_dist = dist * (1.0 + 0.05 * (t * 0.031).sin());
    }
}

/// New game: the title goes, the bars and the clock come back and the view goes to the
/// start.
fn start(world: &mut World) {
    screens::hide(world, TITLE, false);
    screens::hide(world, HELP, true);
    world.resource_mut::<Front>().stage = Stage::Playing;
    // Where the barque and the coast she lies off are both in view (the town's start view
    // when there is no barque: the sandbox).
    let view = first_view(world).or(world.resource::<Front>().start_view);
    {
        let mut q = world.query::<&mut StrategyCam>();
        if let Some(mut cam) = q.iter_mut(world).next() {
            cam.pitch_override = None;
            if let Some((focus, yaw, dist)) = view {
                cam.goal_focus = focus;
                cam.goal_yaw = yaw;
                cam.goal_dist = dist;
            }
        }
    }
    show_bars(world, true);
    if let Some(mut fog) = world.get_resource_mut::<crate::fog::FogOfWar>() {
        fog.shown = true;
    }
    world.resource_mut::<Economy>().speed = 1.0;
    if world.get_resource::<engine_ui::theme::Theme>().is_some() {
        toast::notify(
            world,
            ToastKind::Info,
            "isle_craft_barque",
            &format!("The {} lies off Haverholm", crate::settle::FIRST_SHIP),
            "Open Harbour and air in the build bar and place a Harbour on the coast: the barque \
             sails in with her planks and raises it. F1 shows how to play.",
            14.0,
        );
    }
}

/// Start this program again with the same options (less the start ones) and `extra`, and
/// quit this one.
fn relaunch(world: &mut World, extra: &[&str]) {
    let keep: Vec<String> = std::env::args()
        .skip(1)
        .filter(|a| !matches!(a.as_str(), "--sandbox" | "--title" | "--no-title"))
        .collect();
    let spawned = std::env::current_exe()
        .map_err(|e| e.to_string())
        .and_then(|exe| {
            std::process::Command::new(exe)
                .args(&keep)
                .args(extra)
                .spawn()
                .map_err(|e| e.to_string())
        });
    match spawned {
        Ok(_) => {
            world.write_message(AppExit::Success);
        }
        Err(e) => toast::notify(world, ToastKind::Bad, "", "Could not start again", &e, 6.0),
    }
}

/// Show or hide the game's bars (every `isle_*` screen).
fn show_bars(world: &mut World, on: bool) {
    let want = if on { 1.0 } else { 0.0 };
    let mut q = world.query::<(&Screen, &mut Node)>();
    for (s, mut node) in q.iter_mut(world) {
        if s.key.starts_with("isle_") && node.style.opacity != Some(want) {
            node.style.opacity = Some(want);
        }
    }
}

/// The menu rows' looks (on the theme, so keyboard focus shows like a hover).
fn install_classes(world: &mut World) {
    let Some(mut theme) = world.get_resource_mut::<engine_ui::theme::Theme>() else {
        return;
    };
    let lit = || {
        StylePatch::default()
            .background(Fill::Linear {
                from: Color::hexa(0xd8ab5856),
                to: Color::hexa(0xd8ab5800),
                angle_deg: 0.0,
            })
            .border_color(Color::hexa(0xd8ab58c0))
            .color(Colors::brass_light())
            .translate([8.0, 0.0])
    };
    let row = |size: f32| {
        StylePatch::default()
            .direction(Direction::Row)
            .align_items(Align::Center)
            .gap(18.0)
            .padding([9.0, 22.0, 9.0, 14.0])
            .radius(Corners::all(4.0))
            .border(0.0, Color::hexa(0xd8ab5800))
            .color(Colors::cream())
            .font_family(DISPLAY_FONT)
            .font_weight(700)
            .font_size(size)
            .transition(0.16)
            .ease(Ease::Out)
    };
    for (name, size) in [("fv_item", 31.0), ("fv_item_small", 22.0)] {
        theme.classes.insert(
            name.to_string(),
            ClassStyle {
                base: row(size),
                hover: lit(),
                focused: lit(),
                pressed: lit().translate([10.0, 1.0]),
                ..Default::default()
            },
        );
    }
}

/// A title menu row: icon, name and a line on what it does.
fn item(icon: &str, label: &str, note: &str, action: &str, id: &str) -> El {
    El::new()
        .class("fv_item")
        .id(id)
        .on_click(action)
        .child(
            El::icon(icon)
                .with_style(|s| s.size(40.0, 40.0).shrink(0.0))
                .pick_ignore(),
        )
        .child(
            El::new()
                .with_style(|s| s.direction(Direction::Column).gap(1.0).pick(Pick::Ignore))
                .child(
                    El::text(label)
                        .text_style(|t| t.nowrap().shadow())
                        .pick_ignore(),
                )
                .child(
                    El::text(note)
                        .text_style(|t| t.nowrap().shadow())
                        .with_style(|s| {
                            s.font_family(HUMANIST_FONT)
                                .font_weight(500)
                                .font_size(16.0)
                                .color(Colors::cream_dim())
                        })
                        .pick_ignore(),
                ),
        )
}

/// A key cap: on the brass plate (dark) or on the parchment (light).
fn key(k: &str, on_paper: bool) -> El {
    let (bg, edge, ink) = if on_paper {
        (
            Fill::vertical(Color::hex(0xfbf3dd), Color::hex(0xe6d5ae)),
            Colors::ink_dim(),
            Colors::ink(),
        )
    } else {
        (
            Fill::vertical(Color::hex(0x2a3c4a), Color::hex(0x111b23)),
            Colors::brass_dark(),
            Colors::cream(),
        )
    };
    El::new()
        .with_style(move |s| {
            s.padding([2.0, 9.0, 3.0, 9.0])
                .min_width(30.0)
                .radius(Corners::all(5.0))
                .border(1.0, edge)
                .background(bg)
                .shadow(Shadow::new(Color::hexa(0x00000060), 2.0, 1.5))
                .justify(Justify::Center)
                .align_items(Align::Center)
                .pick(Pick::Ignore)
        })
        .child(
            El::text(k)
                .text_style(|t| t.nowrap().bold().size(15.0))
                .with_style(move |s| s.color(ink).font_family(HUMANIST_FONT))
                .pick_ignore(),
        )
}

/// A key and what it does, for the footers.
fn hint(k: &str, what: &str) -> El {
    El::new()
        .with_style(|s| {
            s.direction(Direction::Row)
                .gap(10.0)
                .align_items(Align::Center)
                .pick(Pick::Ignore)
        })
        .child(key(k, false))
        .child(
            El::text(what)
                .text_style(|t| t.nowrap().shadow())
                .with_style(|s| s.font_size(16.0).color(Colors::cream()))
                .pick_ignore(),
        )
}

fn title_el() -> El {
    let caps = El::text("AN ISLAND TRADING COMPANY")
        .text_style(|t| {
            let mut t = t.nowrap().shadow();
            t.letter_spacing = 0.24;
            t
        })
        .with_style(|s| {
            s.font_family(DISPLAY_CAPS_FONT)
                .font_weight(700)
                .font_size(19.0)
                .color(Colors::brass())
        })
        .pick_ignore();
    let name = El::text(NAME)
        .text_style(|t| t.nowrap().shadow())
        .with_style(|s| {
            s.font_family(DISPLAY_FONT)
                .font_weight(900)
                .font_size(124.0)
                .color(Colors::cream())
                .margin([0.0, 0.0, -6.0, -4.0])
        })
        .pick_ignore();
    let tagline = El::text(TAGLINE)
        .text_style(|t| t.nowrap().shadow())
        .with_style(|s| {
            s.font_family(DISPLAY_FONT)
                .font_weight(700)
                .font_size(25.0)
                .color(Colors::cream_dim())
        })
        .pick_ignore();
    let logo = El::new()
        .with_style(|s| {
            s.direction(Direction::Column)
                .gap(12.0)
                .pick(Pick::Ignore)
                .enter(Enter::rise(0.9, 18.0).ease(Ease::Out))
        })
        .child(caps)
        .child(name)
        .child(brass::divider(340.0))
        .child(tagline);
    let menu = El::new()
        .id("fv_title_menu")
        .with_style(|s| {
            s.direction(Direction::Column)
                .gap(6.0)
                .width(560.0)
                .margin([0.0, 0.0, 0.0, -14.0])
                .enter(Enter::rise(0.8, 24.0).delay(0.35).ease(Ease::Out))
        })
        .child(item(
            "isle_craft_barque",
            "New game",
            "One barque, the Fair Venture, off an empty coast",
            "fv.new",
            "fv_new",
        ))
        .child(item(
            "isle_house",
            "Sandbox town",
            "Four islands and the far lands already at work",
            "fv.sandbox",
            "fv_sandbox",
        ))
        .child(item(
            "isle_compass",
            "How to play",
            "The aim, the first steps and every key",
            "fv.help",
            "fv_title_help",
        ))
        .child(El::new().with_style(|s| {
            s.height(1.0)
                .width(420.0)
                .margin([8.0, 0.0, 8.0, 14.0])
                .background(Fill::Linear {
                    from: Color::hexa(0xd8ab5890),
                    to: Color::hexa(0xd8ab5800),
                    angle_deg: 0.0,
                })
                .pick(Pick::Ignore)
        }))
        .child(item(
            "isle_anchor",
            "Quit",
            "Back to the desktop",
            "fv.quit",
            "fv_quit",
        ));
    let left = El::new()
        .with_style(|s| {
            s.position(Position::Absolute)
                .left(150.0)
                .top(0.0)
                .bottom(0.0)
                .direction(Direction::Column)
                .justify(Justify::Center)
                .gap(54.0)
        })
        .child(logo)
        .child(menu);
    let build = option_env!("GITHUB_SHA").map_or("local", |s| s.get(..7).unwrap_or(s));
    let foot = El::new()
        .with_style(|s| {
            s.position(Position::Absolute)
                .left(150.0)
                .right(90.0)
                .bottom(46.0)
                .direction(Direction::Row)
                .gap(28.0)
                .align_items(Align::Center)
        })
        .child(
            El::text(format!(
                "An island city builder made with the Engine  ·  Rust and wgpu  ·  build {build}"
            ))
            .text_style(|t| t.nowrap().shadow())
            .with_style(|s| s.font_size(15.0).color(Colors::cream_dim()))
            .pick_ignore(),
        )
        .child(El::new().with_style(|s| s.grow(1.0)))
        .child(hint("F1", "How to play"))
        .child(hint("Enter", "Choose"));
    El::new()
        .class("screen")
        .child(El::new().with_style(|s| s.fill_parent().pick(Pick::Block)))
        // A shade behind the words, so they read over sea and sky.
        .child(El::new().with_style(|s| {
            s.position(Position::Absolute)
                .left(0.0)
                .top(0.0)
                .bottom(0.0)
                .width(pct(64.0))
                .background(Fill::Linear {
                    from: Color::hexa(0x050b11e0),
                    to: Color::hexa(0x050b1100),
                    angle_deg: 0.0,
                })
                .pick(Pick::Ignore)
        }))
        .child(El::new().with_style(|s| {
            s.position(Position::Absolute)
                .left(0.0)
                .right(0.0)
                .bottom(0.0)
                .height(240.0)
                .background(Fill::vertical(
                    Color::hexa(0x050b1100),
                    Color::hexa(0x050b11b0),
                ))
                .pick(Pick::Ignore)
        }))
        .child(left)
        .child(foot)
}

// ------------------------------------------------------------------------------ Esc menu

fn open_menu(world: &mut World) {
    install_classes(world);
    let was = world.resource::<Economy>().speed;
    world.resource_mut::<Front>().resume_speed = Some(if was > 0.0 { was } else { 1.0 });
    world.resource_mut::<Economy>().speed = 0.0;
    let screen = Screen {
        key: MENU.into(),
        modal: true,
        layer: 40,
        closable: false,
        ..Default::default()
    };
    if let Err(e) = screens::spawn_screen(world, screen, &menu_el()) {
        engine::core::log::warn!("{NAME} menu: {e}");
    }
}

fn close_menu(world: &mut World) {
    screens::hide(world, MENU, false);
    if let Some(v) = world.resource_mut::<Front>().resume_speed.take() {
        world.resource_mut::<Economy>().speed = v;
    }
}

fn menu_el() -> El {
    let small = |icon: &str, label: &str, action: &str, id: &str| {
        El::new()
            .class("fv_item_small")
            .id(id)
            .on_click(action)
            .child(
                El::icon(icon)
                    .with_style(|s| s.size(30.0, 30.0).shrink(0.0))
                    .pick_ignore(),
            )
            .child(
                El::text(label)
                    .text_style(|t| t.nowrap().shadow())
                    .pick_ignore(),
            )
    };
    let panel = brass::plate()
        .with_style(|s| {
            s.direction(Direction::Column)
                .width(470.0)
                .padding([28.0, 30.0, 30.0, 30.0])
                .gap(6.0)
                // Up, clear of the build bar.
                .margin([0.0, 0.0, 220.0, 0.0])
                .enter(Enter::pop(0.22, 0.96).ease(Ease::Out))
        })
        .child(
            El::new()
                .with_style(|s| {
                    s.direction(Direction::Column)
                        .align_items(Align::Center)
                        .gap(6.0)
                        .margin([0.0, 0.0, 10.0, 0.0])
                        .pick(Pick::Ignore)
                })
                .child(brass::label("THE GAME IS PAUSED", false))
                .child(
                    El::text(NAME)
                        .class("brass_title")
                        .text_style(|t| t.nowrap().shadow())
                        .with_style(|s| s.font_size(40.0))
                        .pick_ignore(),
                )
                .child(brass::divider(220.0)),
        )
        .child(small("ui_arrow_right", "Resume", "fv.resume", "fv_resume"))
        .child(small(
            "isle_compass",
            "How to play",
            "fv.help",
            "fv_menu_help",
        ))
        .child(small(
            "isle_craft_barque",
            "New game",
            "fv.restart",
            "fv_restart",
        ))
        .child(small(
            "isle_anchor",
            "Quit to desktop",
            "fv.quit",
            "fv_menu_quit",
        ))
        .child(
            El::new()
                .with_style(|s| {
                    s.direction(Direction::Row)
                        .justify(Justify::Center)
                        .gap(26.0)
                        .margin([14.0, 0.0, 0.0, 0.0])
                        .pick(Pick::Ignore)
                })
                .child(hint("Esc", "Resume"))
                .child(hint("F1", "How to play")),
        );
    El::new()
        .class("screen center")
        .child(El::new().with_style(|_| engine_ui::widgets::scrim(0.9)))
        .child(panel)
}

// ------------------------------------------------------------------------------ confirm

/// Ask before `action` (a quit, a new game): a brass box with Cancel and `button`.
fn confirm(world: &mut World, title: &str, text: &str, button: &str, action: &str) {
    let screen = Screen {
        key: CONFIRM.into(),
        modal: true,
        layer: 60,
        closable: false,
        ..Default::default()
    };
    let panel = brass::plate()
        .with_style(|s| {
            s.direction(Direction::Column)
                .width(560.0)
                .padding([28.0, 32.0, 28.0, 32.0])
                .gap(16.0)
                .enter(Enter::pop(0.2, 0.96).ease(Ease::Out))
        })
        .child(
            El::text(title)
                .class("brass_title")
                .text_style(|t| t.nowrap().shadow())
                .with_style(|s| s.font_size(30.0))
                .pick_ignore(),
        )
        .child(
            El::text(text)
                .text_style(|t| t.line_height(1.4).shadow())
                .with_style(|s| s.font_size(17.0).color(Colors::cream()))
                .pick_ignore(),
        )
        .child(
            El::new()
                .with_style(|s| {
                    s.direction(Direction::Row)
                        .gap(14.0)
                        .justify(Justify::End)
                        .margin([6.0, 0.0, 0.0, 0.0])
                })
                .child(
                    El::new()
                        .class("brass_button_dark")
                        .id("fv_confirm_cancel")
                        .on_click("fv.confirm.cancel")
                        .child(El::text("Cancel").pick_ignore()),
                )
                .child(
                    El::new()
                        .class("brass_button")
                        .id("fv_confirm_ok")
                        .on_click(format!("fv.confirm.ok:{action}"))
                        .child(El::text(button).pick_ignore()),
                ),
        );
    let el = El::new()
        .class("screen center")
        .child(El::new().with_style(|_| engine_ui::widgets::scrim(0.5)))
        .child(panel);
    screens::hide(world, CONFIRM, true);
    if let Err(e) = screens::spawn_screen(world, screen, &el) {
        engine::core::log::warn!("{NAME} confirm: {e}");
    }
}

// ------------------------------------------------------------------------------ how to play

/// The aim of the game, for the page and PLAY.txt.
pub const AIM: &str = "Your company starts with one barque, the Fair Venture, lying off \
    Haverholm with planks, fish and clothes in her hold. Every other island is hidden under \
    fog. Grow Haverholm from a landing into a town, raise its people from Fisherfolk to \
    Labourers to Burghers, settle the other islands by ship, and bring home the goods of the \
    far lands: Sarrowmere, the Amber Reach and Frostholm.";

/// The first steps of a new game.
pub const STEPS: [&str; 6] = [
    "Place a Harbour on Haverholm's coast (build bar, Harbour and air). The Fair Venture \
     sails in, lands her planks and raises it.",
    "Build a Market square, a Woodcutter by the trees and a Sawmill for planks, and a \
     Fishery on the shore. Join everything to the harbour with roads.",
    "Lay out houses round the market. Fisherfolk move in and pay rent; a house's card says \
     what it is waiting for.",
    "When the town asks for clothes, add a Sheep farm and a Weaver.",
    "A full house with all it needs reads Ready to move up on its card. Pick it and press U \
     to move it up a tier, or Shift+U for every ready house on the island. Labourers want \
     bread, clothes, a market and a chapel; Burghers a school, a tavern and a paved street.",
    "Send a ship with planks to settle another island, then tie the islands together with \
     trade routes (T).",
];

/// Things worth knowing.
pub const TIPS: [&str; 4] = [
    "Idle works still cost their upkeep. Pause or clear the ones you do not need, and watch \
     the balance in the top bar.",
    "Happy towns pay more. Angry ones riot. A Fire Watch, a Watch House and an Infirmary keep \
     fires, riots and sickness in hand.",
    "Each island keeps its own goods in its warehouses, and ships carry them between islands.",
    "The game is not saved yet: a game lasts until you quit.",
];

/// The keys, by group: (key, what it does).
pub const CONTROLS: [(&str, &[(&str, &str)]); 3] = [
    (
        "Camera",
        &[
            ("W A S D", "Pan (arrows too; Shift is faster)"),
            ("Right drag", "Pan with the ground"),
            ("Q  E", "Turn (or middle drag)"),
            ("Wheel", "Zoom, from a street to the whole planet"),
            ("H", "Back to the home island"),
        ],
    ),
    (
        "Building",
        &[
            ("Left click", "Place, or pick a building"),
            ("Drag", "Lay a road with the road tool"),
            ("R", "Turn the building"),
            ("U", "Move the picked house up a tier"),
            ("Shift+U", "Move up every ready house the works can spare"),
            ("Right click", "Put the tool down (Esc too)"),
            ("Delete", "Demolish"),
        ],
    ),
    (
        "Company",
        &[
            ("Space", "Pause"),
            ("1  2  3", "Speed: normal, double, four times"),
            ("L  T", "Ledger, trade routes"),
            ("G  I", "Fleet, items"),
            ("F1  Esc", "This page, the menu"),
            ("F11", "Window or fullscreen"),
        ],
    ),
];

fn open_help(world: &mut World) {
    install_classes(world);
    let screen = Screen {
        key: HELP.into(),
        modal: true,
        layer: 50,
        closable: false,
        ..Default::default()
    };
    screens::hide(world, HELP, true);
    if let Err(e) = screens::spawn_screen(world, screen, &help_el()) {
        engine::core::log::warn!("{NAME} help: {e}");
    }
}

fn help_el() -> El {
    let heading = |text: &str| {
        El::new()
            .with_style(|s| {
                s.direction(Direction::Row)
                    .align_items(Align::Center)
                    .gap(12.0)
                    .margin([6.0, 0.0, 2.0, 0.0])
                    .pick(Pick::Ignore)
            })
            .child(
                El::text(text.to_uppercase())
                    .class("ink_label")
                    .text_style(|t| {
                        let mut t = t.nowrap();
                        t.letter_spacing = 0.16;
                        t
                    })
                    .with_style(|s| s.font_size(15.0))
                    .pick_ignore(),
            )
            .child(El::new().with_style(|s| {
                s.height(1.0)
                    .grow(1.0)
                    .background(Color::hexa(0x6a533960))
                    .pick(Pick::Ignore)
            }))
    };
    let para = |text: &str| {
        El::text(text)
            .text_style(|t| t.line_height(1.42))
            .with_style(|s| s.font_size(17.0).color(Colors::ink()))
            .pick_ignore()
    };
    let numbered = |n: usize, text: &str| {
        El::new()
            .with_style(|s| {
                s.direction(Direction::Row)
                    .gap(12.0)
                    .align_items(Align::Start)
                    .pick(Pick::Ignore)
            })
            .child(
                El::new()
                    .with_style(|s| {
                        s.size(26.0, 26.0)
                            .shrink(0.0)
                            .radius(Corners::all(13.0))
                            .background(Fill::vertical(Color::hex(0xf2d692), Color::hex(0xa7772d)))
                            .border(1.0, Colors::outline())
                            .justify(Justify::Center)
                            .align_items(Align::Center)
                            .pick(Pick::Ignore)
                    })
                    .child(
                        El::text(n.to_string())
                            .text_style(|t| t.nowrap().bold().size(14.0))
                            .with_style(|s| s.color(Colors::ink()))
                            .pick_ignore(),
                    ),
            )
            .child(
                El::text(text)
                    .text_style(|t| t.line_height(1.38))
                    .with_style(|s| s.font_size(16.0).color(Colors::ink()).grow(1.0).shrink(1.0))
                    .pick_ignore(),
            )
    };
    let bullet = |text: &str| {
        El::new()
            .with_style(|s| {
                s.direction(Direction::Row)
                    .gap(12.0)
                    .align_items(Align::Start)
                    .pick(Pick::Ignore)
            })
            .child(
                El::text("◆")
                    .text_style(|t| t.nowrap().size(11.0))
                    .with_style(|s| s.color(Colors::brass_dark()).margin([5.0, 6.0, 0.0, 7.0]))
                    .pick_ignore(),
            )
            .child(
                El::text(text)
                    .text_style(|t| t.line_height(1.38))
                    .with_style(|s| s.font_size(16.0).color(Colors::ink()).grow(1.0).shrink(1.0))
                    .pick_ignore(),
            )
    };
    let mut left = El::new()
        .with_style(|s| {
            s.direction(Direction::Column)
                .gap(10.0)
                .width(640.0)
                .shrink(0.0)
                .pick(Pick::Ignore)
        })
        .child(heading("The aim"))
        .child(para(AIM))
        .child(heading("First steps"));
    for (i, s) in STEPS.iter().enumerate() {
        left = left.child(numbered(i + 1, s));
    }
    left = left.child(heading("Good to know"));
    for s in TIPS {
        left = left.child(bullet(s));
    }
    let mut right = El::new().with_style(|s| {
        s.direction(Direction::Column)
            .gap(7.0)
            .width(470.0)
            .shrink(0.0)
            .pick(Pick::Ignore)
    });
    for (group, rows) in CONTROLS {
        right = right.child(heading(group));
        for (k, what) in rows {
            let caps = El::new()
                .with_style(|s| {
                    s.direction(Direction::Row)
                        .gap(5.0)
                        .width(170.0)
                        .shrink(0.0)
                        .justify(Justify::End)
                        .pick(Pick::Ignore)
                })
                .children(k.split("  ").map(|one| key(one, true)));
            right = right.child(
                El::new()
                    .with_style(|s| {
                        s.direction(Direction::Row)
                            .gap(14.0)
                            .align_items(Align::Center)
                            .pick(Pick::Ignore)
                    })
                    .child(caps)
                    .child(
                        El::text(*what)
                            .text_style(|t| t.nowrap())
                            .with_style(|s| s.font_size(16.0).color(Colors::ink()))
                            .pick_ignore(),
                    ),
            );
        }
    }
    let close = El::new()
        .class("brass_button")
        .id("fv_help_close")
        .on_click("fv.help.close")
        .child(El::text("Close").pick_ignore())
        .child(key("Esc", true));
    let head = El::new()
        .with_style(|s| {
            s.direction(Direction::Row)
                .align_items(Align::Center)
                .gap(18.0)
        })
        .child(
            El::new()
                .with_style(|s| s.direction(Direction::Column).gap(2.0).pick(Pick::Ignore))
                .child(brass::label("HOW TO PLAY", true))
                .child(
                    El::text(NAME)
                        .class("ink_title")
                        .text_style(|t| t.nowrap())
                        .with_style(|s| s.font_size(40.0).font_weight(900))
                        .pick_ignore(),
                ),
        )
        .child(El::new().with_style(|s| s.grow(1.0).pick(Pick::Ignore)))
        .child(close);
    let card = brass::parchment()
        .with_style(|s| {
            s.direction(Direction::Column)
                .width(1290.0)
                .padding([30.0, 44.0, 36.0, 44.0])
                .gap(14.0)
                .enter(Enter::rise(0.3, 22.0).ease(Ease::Out))
        })
        .child(head)
        .child(
            El::new()
                .with_style(|s| {
                    s.direction(Direction::Row)
                        .gap(56.0)
                        .align_items(Align::Start)
                        .pick(Pick::Ignore)
                })
                .child(left)
                .child(right),
        );
    El::new()
        .class("screen center")
        .child(El::new().with_style(|_| engine_ui::widgets::scrim(0.72)))
        .child(card)
}

// ------------------------------------------------------------------------------ pictures

/// `--title-shots`: the title, the page, the start of a new game and the Esc menu, as
/// pictures in `screenshots/archipelago/title/`.
pub fn shots(mut app: App) {
    let out = "screenshots/archipelago/title";
    let _ = std::fs::create_dir_all(out);
    app.world.resource_mut::<EngineConfig>().headless = true;
    if let Some(mut i) = app.world.get_resource_mut::<crate::incidents::Incidents>() {
        i.rolls = false;
    }
    app.world
        .resource_mut::<Time>()
        .set_source(engine::core::time::TimeSource::Manual {
            step_secs: 1.0 / 30.0,
        });
    app.world.resource_mut::<Front>().stage = Stage::Title;
    app.update_n(3);
    let cap = std::env::var("ARCH_TOUR_SETTLE_S")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(240.0);
    let only = std::env::var("FV_SHOTS").ok();
    let wants = |k: &str| only.as_deref().is_none_or(|o| o.split(',').any(|x| x == k));
    let snap = |app: &mut App, key: &str, frames: usize| {
        settle(app, cap);
        app.update_n(frames);
        if let Some(d) = app.world.get_resource::<RenderDisabled>() {
            eprintln!("no GPU: {}", d.0);
            return;
        }
        let path = format!("{out}/{key}.png");
        match capture_now(&mut app.world, &path) {
            Ok(_) => engine::core::log::info!("{NAME} shots: {path}"),
            Err(e) => engine::core::log::warn!("{NAME} shots: {path}: {e}"),
        }
    };
    if wants("title") {
        snap(&mut app, "title", 40);
    }
    if wants("help") {
        open_help(&mut app.world);
        snap(&mut app, "help", 20);
        screens::hide(&mut app.world, HELP, true);
    }
    start(&mut app.world);
    {
        let mut q = app.world.query::<&mut StrategyCam>();
        if let Some(mut cam) = q.iter_mut(&mut app.world).next() {
            cam.snap();
        }
    }
    if wants("start") {
        snap(&mut app, "start", 20);
    }
    if wants("menu") {
        open_menu(&mut app.world);
        snap(&mut app, "menu", 20);
    }
}

/// Frames until the terrain and the trees are in, at most `cap_s` wall seconds.
fn settle(app: &mut App, cap_s: f32) {
    let started = std::time::Instant::now();
    loop {
        app.update();
        let w = &app.world;
        let (building, missing) = w
            .get_resource::<engine::core::diagnostics::Diagnostics>()
            .map_or((0.0, 0.0), |d| {
                (
                    d.get("terrain.patches_building").unwrap_or(0.0),
                    d.get("terrain.patches_missing").unwrap_or(0.0),
                )
            });
        let foliage = w
            .get_resource::<engine_foliage::FoliageLoading>()
            .is_some_and(|f| !f.is_ready());
        let elapsed = started.elapsed().as_secs_f32();
        if building + missing == 0.0 && !foliage && elapsed > 0.5 {
            break;
        }
        if elapsed > cap_s {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(60));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(s: &str) -> String {
        s.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    /// The zip's PLAY.txt says what the in-game page says.
    #[test]
    fn play_txt_matches_the_page() {
        let txt = flat(include_str!("../PLAY.txt"));
        assert!(txt.starts_with(NAME), "PLAY.txt should open with the name");
        let mut lines: Vec<String> = vec![flat(AIM)];
        lines.extend(STEPS.iter().map(|s| flat(s)));
        lines.extend(TIPS.iter().map(|s| flat(s)));
        for (_, rows) in CONTROLS {
            lines.extend(rows.iter().map(|(k, what)| flat(&format!("{k} {what}"))));
        }
        for l in lines {
            assert!(txt.contains(&l), "PLAY.txt lacks: {l}");
        }
    }

    /// The first view keeps the barque above the build bar and the coast in sight, however
    /// far out she lies.
    #[test]
    fn first_view_frames_the_barque_and_the_coast() {
        let fov = 50f32.to_radians();
        for len in [420.0, 520.0, 700.0, 900.0, 1200.0] {
            let (f, dist) = frame_two(len, fov, BARQUE_Y, COAST_Y);
            let barque = screen_y(-f * len, dist, fov);
            let coast = screen_y((1.0 - f) * len, dist, fov);
            assert!(
                (-0.22..-0.1).contains(&barque),
                "{len} m out: barque at {barque:.2}"
            );
            assert!(
                (0.6..0.78).contains(&coast),
                "{len} m out: coast at {coast:.2}"
            );
        }
    }
}
