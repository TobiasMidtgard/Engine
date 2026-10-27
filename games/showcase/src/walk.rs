//! The exe's start: a start screen over the valley (the camera drifting round it), then either
//! a short walk through the best stops on foot, or the free camera.
//!
//! The walk puts the play-mode pilot (Movement's) on the plaza and leads them stop by stop: a
//! marker over the next stop, pinned to the screen edge while it is out of view, and a card
//! with what to look at there. Reaching a stop moves the marker on. After the last one, P
//! leaves play mode for the free camera, whose 1-9, 0, O and M keys jump around the world.
//!
//! Tours, films and `--fly` skip it all.

use crate::camera::{self, ShowcaseCamera};
use crate::landscape;
use bevy_ecs::message::MessageCursor;
use engine::prelude::*;
use engine_ui::color::Color;
use engine_ui::el::El;
use engine_ui::hud::{Marker, spawn_marker};
use engine_ui::input::UiEvent;
use engine_ui::node::{Screen, WorldAnchor};
use engine_ui::screens;
use engine_ui::style::{Align, Fill, Justify, Position, pct};
use engine_ui::theme::Theme;
use engine_ui::toast::{self, ToastKind};
use serde_json::json;

/// One stop on the walk.
pub struct Stop {
    pub title: &'static str,
    /// What to look at or try there.
    pub hint: &'static str,
    /// Where the marker stands (x, z on the valley floor).
    pub at: [f64; 2],
}

/// The walk, from the plaza: the village market, the cloth yard, down the worn footpath to the
/// lake, round its shore to the waterfall and on to the herds on the meadow (about 600 m on
/// foot).
pub const STOPS: [Stop; 6] = [
    Stop {
        title: "The market street",
        hint: "Stalls, traders and villagers going about their day.",
        at: [58.0, 50.0],
    },
    Stop {
        title: "The cloth yard",
        hint: "Flags, sails and a rope bridge in the wind. Try crossing the bridge.",
        at: [104.0, -22.0],
    },
    Stop {
        title: "The footpath",
        hint: "A path the villagers wore down to the lake, bare where they walk most.",
        at: [154.0, -110.0],
    },
    Stop {
        title: "The lake shore",
        hint: "The wind ruffles the water and wakes spread behind anything that moves.",
        at: [138.0, -205.0],
    },
    Stop {
        title: "The waterfall",
        hint: "The stream drops over the cliff into a pool. Listen to it.",
        at: [248.0, -289.0],
    },
    Stop {
        title: "The meadow",
        hint: "Deer and rabbits graze by the shore. Walk up slowly, or run at them.",
        at: [338.0, -218.0],
    },
];

/// Reaching within this of a stop (m, on the ground) counts.
const REACH_M: f64 = 14.0;
/// Where the pilot starts: on the plaza, facing the market.
const SPAWN: [f64; 2] = [8.0, 18.0];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Stage {
    /// The start screen is up.
    #[default]
    Menu,
    /// Walking: heading for this stop.
    Walking(usize),
    /// Free camera (or the walk is over).
    Free,
}

#[derive(Resource, Default)]
pub struct Walk {
    stage: Stage,
    shown: bool,
    /// The camera drifts round the valley behind the start screen (not when a tour or an agent
    /// opened it: they place the camera).
    drift: bool,
    marker: Option<Entity>,
    /// Real seconds on the start screen (the camera's drift).
    clock: f32,
}

/// The walk's systems and APIs; `start_screen` opens the start screen at once (the exe), else
/// it waits for `showcase.start_screen` or `showcase.walk` (tours, agents).
pub fn install(app: &mut App, start_screen: bool) {
    app.insert_resource(Walk {
        stage: if start_screen {
            Stage::Menu
        } else {
            Stage::Free
        },
        drift: start_screen,
        ..Default::default()
    })
    .add_systems(Update, run.after(camera::fly))
    .register_api(
        "showcase.start_screen",
        "Open the exe's start screen (Walk the valley, Fly freely, Quit) over the current view.",
        "{}",
        |world, _| {
            let mut walk = world.remove_resource::<Walk>().ok_or("no walk")?;
            let shown = show_menu(world);
            if shown.is_ok() {
                walk.stage = Stage::Menu;
                walk.shown = true;
            }
            world.insert_resource(walk);
            shown.map(|_| json!({"open": true}))
        },
    )
    .register_api(
        "showcase.walk",
        "Start the valley walk: the play-mode pilot on the plaza, heading for \"stop\" (1-6, \
         default 1) with its marker and card.",
        r#"{"stop": 1}"#,
        |world, args| {
            let stop = args.get("stop").and_then(|v| v.as_u64()).unwrap_or(1) as usize;
            if !(1..=STOPS.len()).contains(&stop) {
                return Err(format!("\"stop\" is 1 to {}", STOPS.len()));
            }
            let mut walk = world.remove_resource::<Walk>().ok_or("no walk")?;
            start_walk(world, &mut walk, stop - 1);
            let walking = matches!(walk.stage, Stage::Walking(_));
            world.insert_resource(walk);
            Ok(json!({"walking": walking, "stop": STOPS[stop - 1].title}))
        },
    );
}

fn ground(x: f64, z: f64) -> DVec3 {
    DVec3::new(x, landscape::shared().height(x, z), z)
}

fn run(world: &mut World, mut seen: Local<MessageCursor<UiEvent>>) {
    let events = engine_ui::input::read_events(world, &mut seen);
    let mut walk = world.remove_resource::<Walk>().expect("installed");
    for ev in &events {
        let UiEvent::Action { action, .. } = ev else {
            continue;
        };
        match action.as_str() {
            "walk.start" if walk.stage == Stage::Menu => start_walk(world, &mut walk, 0),
            "walk.fly" if walk.stage == Stage::Menu => {
                screens::hide(world, "walk_menu", false);
                walk.stage = Stage::Free;
            }
            "walk.quit" => {
                world.write_message(AppExit::Success);
            }
            _ => {}
        }
    }
    match walk.stage {
        Stage::Menu => {
            if !walk.shown && !crate::build::building(world) {
                walk.shown = true;
                if let Err(e) = show_menu(world) {
                    engine::core::log::warn!("start screen: {e}");
                    walk.stage = Stage::Free;
                }
            }
            if walk.drift {
                walk.clock += world.resource::<Time>().real_delta_secs().min(0.1);
                drift(world, walk.clock);
            }
        }
        Stage::Walking(i) => {
            let play = *world.resource::<engine_movement::Play>();
            let feet = play
                .pilot
                .and_then(|p| world.get::<GlobalTransform>(p))
                .map(|t| t.translation());
            if let Some(feet) = feet {
                let stop = &STOPS[i];
                let d = DVec2::new(feet.x - stop.at[0], feet.z - stop.at[1]).length();
                if d < REACH_M {
                    arrive(world, &mut walk, i);
                }
            }
        }
        Stage::Free => {}
    }
    world.insert_resource(walk);
}

/// The camera drifts slowly round the valley behind the start screen.
fn drift(world: &mut World, t: f32) {
    let (eye, target) = start_view(t);
    put_camera(world, eye, target);
}

/// The start screen's view `t` seconds in: high over the village, circling slowly.
pub fn start_view(t: f32) -> (DVec3, DVec3) {
    let a = 2.2 + t as f64 * 0.02;
    let center = DVec3::new(90.0, 0.0, -40.0);
    let eye = center + DVec3::new(a.cos() * 230.0, 0.0, a.sin() * 230.0);
    let ground = landscape::shared().height(eye.x, eye.z).max(0.0);
    (
        DVec3::new(eye.x, ground + 55.0, eye.z),
        center + DVec3::new(0.0, 8.0, 0.0),
    )
}

/// Where the walk heading for stop `i` starts, and the stop (for tour viewpoints).
pub fn leg(i: usize) -> (DVec3, DVec3) {
    let from = if i == 0 { SPAWN } else { STOPS[i - 1].at };
    (
        ground(from[0], from[1]),
        ground(STOPS[i].at[0], STOPS[i].at[1]),
    )
}

fn put_camera(world: &mut World, eye: DVec3, target: DVec3) {
    let mut q = world.query::<(&mut Transform, &mut GlobalTransform, &mut ShowcaseCamera)>();
    for (mut tr, mut global, mut cam) in q.iter_mut(world) {
        camera::look(&mut tr, &mut cam, eye, target);
        // Now, not after this frame's transform pass: Movement's pilot takes its facing from
        // the camera's view the moment play starts (the camera has no parent).
        *global = GlobalTransform::from(*tr);
    }
}

fn menu_item(icon: &str, label: &str, action: &str, id: &str) -> El {
    El::new()
        .class("menu_item")
        .id(id)
        .on_click(action)
        .child(
            El::icon(icon)
                .with_style(|s| s.width(22.0).height(22.0))
                .pick_ignore(),
        )
        .child(
            El::text(label)
                .text_style(|t| t.nowrap())
                .with_style(|s| s.grow(1.0))
                .pick_ignore(),
        )
}

/// A band of the start screen's dark wash: `left` and `width` in percent of the view, its
/// opacity going from `from` to `to` left to right.
fn wash(left: f32, width: f32, from: f32, to: f32) -> El {
    let shade = |a| Color::linear(0.01, 0.015, 0.025, a);
    El::new()
        .with_style(|s| {
            s.position(Position::Absolute)
                .left(pct(left))
                .top(0.0)
                .bottom(0.0)
                .width(pct(width))
                .background(Fill::Linear {
                    from: shade(from),
                    to: shade(to),
                    angle_deg: 0.0,
                })
        })
        .pick_ignore()
}

fn show_menu(world: &mut World) -> Result<(), String> {
    let p = world.resource::<Theme>().palette.clone();
    let menu = El::new()
        .class("col")
        .id("walk_menu_items")
        .with_style(|s| s.gap(4.0).width(420.0))
        .child(menu_item(
            "ui_arrow_right",
            "Walk the valley",
            "walk.start",
            "walk_start",
        ))
        .child(menu_item(
            "map_scroll",
            "Fly freely",
            "walk.fly",
            "walk_fly",
        ))
        .child(
            El::new()
                .class("divider")
                .with_style(|s| s.margin([8.0, 0.0])),
        )
        .child(menu_item("ui_close", "Quit", "walk.quit", "walk_quit"));
    let logo = El::new()
        .class("col")
        .with_style(|s| s.gap(8.0))
        .child(
            El::text("RUST  +  WGPU  ENGINE")
                .text_style(|t| t.bold().nowrap())
                .with_style(|s| s.font_size(13.0).color(p.accent)),
        )
        .child(
            El::text("THE VALLEY")
                .text_style(|t| t.bold().nowrap().shadow())
                .with_style(|s| s.font_size(96.0).color(Color::WHITE)),
        )
        .child(
            El::text("A village on a real planet, from the meadow to orbit.")
                .text_style(|t| t.nowrap().shadow())
                .with_style(|s| s.font_size(18.0).font_weight(600).color(p.text_dim)),
        );
    let el = El::new()
        .with_style(|s| {
            s.position(Position::Absolute)
                .left(0.0)
                .right(0.0)
                .top(0.0)
                .bottom(0.0)
        })
        // A dark wash from the left edge so the title and menu read over a bright valley:
        // deep behind them, fading out across the middle of the view.
        .child(wash(0.0, 42.0, 0.9, 0.72))
        .child(wash(42.0, 38.0, 0.72, 0.0))
        .child(
            El::new()
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
                .child(menu),
        )
        .child(
            El::new()
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
                    El::text("On foot: WASD, mouse, Space to jump, Shift to sprint. P switches to the free camera.")
                        .class("caption")
                        .text_style(|t| t.nowrap()),
                ),
        );
    screens::spawn_screen(
        world,
        Screen {
            key: "walk_menu".into(),
            modal: true,
            layer: 18,
            closable: false,
            ..Default::default()
        },
        &el,
    )?;
    Ok(())
}

/// Put the pilot on the ground heading for stop `i`: on the plaza for the first, at the stop
/// before for the others (tours and agents start part way).
fn start_walk(world: &mut World, walk: &mut Walk, i: usize) {
    screens::hide(world, "walk_menu", false);
    // Look from behind the start toward the stop, so the pilot starts facing it.
    let (feet, next) = leg(i);
    let back = (feet - next).with_y(0.0).normalize_or(DVec3::Z);
    put_camera(
        world,
        feet + back * 7.0 + DVec3::Y * 2.5,
        feet + DVec3::Y * 1.5,
    );
    let at = [feet.x, feet.y, feet.z];
    if let Err(e) =
        engine::core::registry::call_api(world, "movement.play", json!({"on": true, "at": at}))
    {
        engine::core::log::warn!("walk: no pilot ({e}); free camera instead");
        walk.stage = Stage::Free;
        return;
    }
    walk.stage = Stage::Walking(i);
    point_at(world, walk, i);
}

/// Move the marker and the card to stop `i`.
fn point_at(world: &mut World, walk: &mut Walk, i: usize) {
    if let Some(old) = walk.marker.take()
        && let Ok(e) = world.get_entity_mut(old)
    {
        e.despawn();
    }
    let stop = &STOPS[i];
    let marker = Marker {
        title: stop.title.into(),
        always: true,
        ..Default::default()
    };
    let anchor = WorldAnchor {
        position: ground(stop.at[0], stop.at[1]) + DVec3::Y * 3.0,
        clamp_to_edges: true,
        ..Default::default()
    };
    walk.marker = spawn_marker(world, &marker, anchor).ok();
    show_card(
        world,
        &format!("{} of {}", i + 1, STOPS.len()),
        stop.title,
        stop.hint,
    );
}

fn arrive(world: &mut World, walk: &mut Walk, i: usize) {
    toast::notify(
        world,
        ToastKind::Good,
        "star",
        STOPS[i].title,
        STOPS[i].hint,
        4.0,
    );
    if i + 1 < STOPS.len() {
        walk.stage = Stage::Walking(i + 1);
        point_at(world, walk, i + 1);
        return;
    }
    if let Some(old) = walk.marker.take()
        && let Ok(e) = world.get_entity_mut(old)
    {
        e.despawn();
    }
    walk.stage = Stage::Free;
    show_card(
        world,
        "Walk done",
        "Carry on exploring",
        "P switches to the free camera. Then 1-9 jump to the zones, 0 to the overview, O to orbit and M to the Moon.",
    );
}

/// The card in the top right: where the walk is and what to look at.
fn show_card(world: &mut World, step: &str, title: &str, hint: &str) {
    screens::hide(world, "walk_card", true);
    let p = world.resource::<Theme>().palette.clone();
    let el = El::new()
        .class("panel")
        .with_style(|s| {
            s.position(Position::Absolute)
                .right(36.0)
                .top(120.0)
                .width(340.0)
                .padding([14.0, 18.0])
                .gap(6.0)
        })
        .child(
            El::text(format!("WALK THE VALLEY  ·  {}", step.to_uppercase()))
                .text_style(|t| t.bold().nowrap())
                .with_style(|s| s.font_size(12.0).color(p.accent)),
        )
        .child(El::text(title).class("heading"))
        .child(El::text(hint).with_style(|s| s.color(p.text_dim)));
    if let Err(e) = screens::spawn_screen(
        world,
        Screen {
            key: "walk_card".into(),
            modal: false,
            layer: 4,
            closable: false,
            ..Default::default()
        },
        &el,
    ) {
        engine::core::log::warn!("walk card: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_walk_stays_on_dry_ground_within_reach() {
        let land = landscape::shared();
        let mut from = DVec2::from_array(SPAWN);
        let mut total = 0.0;
        for stop in &STOPS {
            let at = DVec2::from_array(stop.at);
            assert!(
                land.lake_t(at.x, at.y) > 1.15,
                "{} is in the lake",
                stop.title
            );
            assert!(
                land.stream_at(at.x, at.y).is_none_or(|(d, _)| d > 4.0),
                "{} is in the stream",
                stop.title
            );
            assert!(
                land.normal(at.x, at.y, 1.0).y > 0.9,
                "{} is on a steep slope",
                stop.title
            );
            // Round the lake, not through it (the beach is fine).
            for k in 0..=20 {
                let q = from.lerp(at, k as f64 / 20.0);
                assert!(
                    land.lake_t(q.x, q.y) > 0.97,
                    "the way to {} crosses the lake at {q}",
                    stop.title
                );
            }
            total += (at - from).length();
            from = at;
        }
        assert!(total < 900.0, "{total:.0} m on foot");
    }
}
