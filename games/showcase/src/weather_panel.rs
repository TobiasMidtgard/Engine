//! The weather panel (F4): presets, hand-set cloud cover, cloud shape, rain, wind, temperature
//! and fog, the wind's direction, the hour and how fast the weather runs (playtest 3 item 32,
//! "better weather controls, preferably a UI").
//!
//! It is a thin skin over the weather module's `weather.panel` (what can be set, its range and
//! its value now) and `weather.tune` / `weather.set` / `weather.time` APIs, built from
//! engine_ui's stock widgets. The UI module may replace it with a themed panel; the APIs stay.
//!
//! Sliders show the weather as it is and follow it while untouched; moving one hands that value
//! to you until "Let the weather decide". Arrows move between rows and Left/Right adjust when
//! walking (the mouse stays with the camera); in free flight the mouse drags them.

use bevy_ecs::message::MessageCursor;
use engine::prelude::*;
use engine_ui::data::UiData;
use engine_ui::el::El;
use engine_ui::input::UiEvent;
use engine_ui::node::Screen;
use engine_ui::screens;
use engine_ui::style::Position;
use engine_ui::widgets::{self, Slider};
use serde_json::{Value, json};

/// The screen's key, also the root of its values in [`UiData`].
const KEY: &str = "weather_panel";
/// A preset chosen here eases in over this long (s).
const PRESET_EASE_S: f32 = 20.0;
/// How often untouched sliders catch up with the weather (s).
const REFRESH_S: f32 = 0.5;

/// What the panel last showed or sent for each value, by name; a [`UiData`] value that differs
/// was moved by hand.
#[derive(Resource, Default)]
struct Panel {
    open: bool,
    shown: Vec<(String, f64)>,
    refresh: f32,
}

pub fn install(app: &mut App) {
    app.init_resource::<Panel>().add_systems(
        Update,
        (toggle_panel, run_panel.run_if(|p: Res<Panel>| p.open)),
    );
}

/// F4 opens and closes the panel. Queues the change only on the frame the key goes down, so
/// the frame keeps no exclusive system for it (the lead's schedule audit).
fn toggle_panel(mut commands: Commands, keys: Option<Res<ButtonInput<KeyCode>>>) {
    if keys.is_some_and(|k| k.just_pressed(KeyCode::F4)) {
        commands.queue(toggle);
    }
}

fn toggle(world: &mut World) {
    if screens::hide(world, KEY, false) {
        world.resource_mut::<Panel>().open = false;
        return;
    }
    if let Err(e) = open(world) {
        engine::core::log::warn!("weather panel: {e}");
        engine_ui::toast::notify(
            world,
            engine_ui::toast::ToastKind::Bad,
            "",
            "Weather",
            &e,
            4.0,
        );
    }
}

/// One slider: the name it is read and sent by, its label and range.
struct Row {
    name: String,
    label: String,
    unit: String,
    range: [f32; 3],
    value: f64,
}

/// The rows from `weather.panel`: the hand-settable values, then wind direction, the hour and
/// the weather's speed.
fn rows(panel: &Value) -> Vec<Row> {
    let mut rows: Vec<Row> = panel["tweaks"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|t| Row {
            name: t["name"].as_str().unwrap_or_default().to_string(),
            label: t["label"].as_str().unwrap_or_default().to_string(),
            unit: t["unit"].as_str().unwrap_or_default().to_string(),
            range: [
                t["min"].as_f64().unwrap_or(0.0) as f32,
                t["max"].as_f64().unwrap_or(1.0) as f32,
                t["step"].as_f64().unwrap_or(0.05) as f32,
            ],
            value: t["set"].as_f64().or(t["now"].as_f64()).unwrap_or(0.0),
        })
        .collect();
    let extra = [
        (
            "wind_from_deg",
            "Wind from",
            "°",
            [0.0, 355.0, 5.0],
            &panel["wind_from_deg"],
        ),
        (
            "hour",
            "Hour",
            " h",
            [0.0, 23.75, 0.25],
            &panel["clock"]["hour"],
        ),
        (
            "time_scale",
            "Weather speed",
            "×",
            [0.0, 60.0, 1.0],
            &panel["time_scale"],
        ),
    ];
    for (name, label, unit, range, v) in extra {
        rows.push(Row {
            name: name.into(),
            label: label.into(),
            unit: unit.into(),
            range,
            value: v.as_f64().unwrap_or(0.0),
        });
    }
    rows
}

fn path(name: &str) -> String {
    format!("{KEY}.{name}")
}

fn decimals(step: f32) -> i32 {
    if step >= 1.0 {
        0
    } else if step >= 0.1 {
        1
    } else {
        2
    }
}

fn open(world: &mut World) -> Result<(), String> {
    let panel = engine::core::registry::call_api(world, "weather.panel", json!({}))?;
    let rows = rows(&panel);
    {
        let mut data = world.resource_mut::<UiData>();
        for r in &rows {
            data.set(&path(&r.name), r.value);
        }
    }
    let presets: Vec<El> = panel["presets"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|p| {
            let name = p["name"].as_str().unwrap_or_default();
            let label = p["label"].as_str().unwrap_or(name);
            El::button(label, format!("{KEY}.preset:{name}")).class("ghost")
        })
        .collect();
    let sliders: Vec<El> = rows
        .iter()
        .map(|r| {
            // Units the value label carries itself (250°, 15.0 h, 4×); the rest in the label.
            let in_value = r.unit.starts_with(' ') || r.unit == "°" || r.unit == "×";
            let label = if r.unit.is_empty() || in_value {
                r.label.clone()
            } else {
                format!("{} ({})", r.label, r.unit)
            };
            let suffix = if in_value {
                r.unit.clone()
            } else {
                String::new()
            };
            widgets::slider_row(
                &label,
                Slider {
                    value: r.value as f32,
                    min: r.range[0],
                    max: r.range[1],
                    step: r.range[2],
                    bind: path(&r.name),
                    decimals: decimals(r.range[2]),
                    suffix,
                },
            )
        })
        .collect();
    let body = El::new()
        .class("panel")
        .with_style(|s| {
            s.position(Position::Absolute)
                .right(28.0)
                .top(28.0)
                .width(560.0)
                .gap(12.0)
                .padding(22.0)
        })
        .child(widgets::header(
            None,
            "Weather",
            "Presets ease in; moved sliders hold until handed back. F4 closes.",
            true,
        ))
        .child(El::text("Presets").class("label_caps"))
        .child(
            El::new()
                .class("row")
                .with_style(|s| s.gap(8.0).wrap(true))
                .children(presets),
        )
        .child(widgets::divider())
        .children(sliders)
        .child(widgets::divider())
        .child(El::button("Let the weather decide", format!("{KEY}.auto")));
    let root = El::new()
        .with_style(|s| s.fill_parent().pick(engine_ui::style::Pick::Ignore))
        .child(body);
    screens::spawn_screen(
        world,
        Screen {
            key: KEY.into(),
            modal: true,
            layer: 40,
            closable: true,
            ..Default::default()
        },
        &root,
    )?;
    let mut p = world.resource_mut::<Panel>();
    p.open = true;
    p.shown = rows.iter().map(|r| (r.name.clone(), r.value)).collect();
    p.refresh = REFRESH_S;
    Ok(())
}

/// Send what was moved, run the buttons, and let untouched sliders follow the weather.
fn run_panel(world: &mut World, mut seen: Local<MessageCursor<UiEvent>>) {
    if screens::find_screen(world, KEY).is_none() {
        // Closed with Escape or the header's button.
        world.resource_mut::<Panel>().open = false;
        return;
    }
    let call = |world: &mut World, api: &str, args: Value| {
        if let Err(e) = engine::core::registry::call_api(world, api, args) {
            engine::core::log::warn!("weather panel: {api}: {e}");
        }
    };
    let mut reopen = false;
    for ev in engine_ui::input::read_events(world, &mut seen) {
        let UiEvent::Action { action, .. } = ev else {
            continue;
        };
        if let Some(preset) = action.strip_prefix(&format!("{KEY}.preset:")) {
            call(
                world,
                "weather.set",
                json!({"preset": preset, "transition_s": PRESET_EASE_S}),
            );
        } else if action == format!("{KEY}.auto") {
            call(world, "weather.tune", json!({"reset": true}));
            reopen = true;
        }
    }
    if reopen {
        // Fresh sliders showing the weather's own values.
        screens::hide(world, KEY, true);
        world.resource_mut::<Panel>().open = false;
        toggle(world);
        return;
    }
    let shown = std::mem::take(&mut world.resource_mut::<Panel>().shown);
    let mut next = Vec::with_capacity(shown.len());
    for (name, before) in shown {
        let now = world
            .resource::<UiData>()
            .get_f64(&path(&name))
            .unwrap_or(before);
        if (now - before).abs() > 1e-6 {
            let args = json!({ name.as_str(): now });
            match name.as_str() {
                "hour" => call(world, "weather.time", args),
                _ => call(world, "weather.tune", args),
            }
        }
        next.push((name, now));
    }
    let refresh = {
        let dt = world.resource::<Time>().real_delta_secs();
        let mut p = world.resource_mut::<Panel>();
        p.refresh -= dt;
        p.refresh <= 0.0
    };
    if refresh {
        world.resource_mut::<Panel>().refresh = REFRESH_S;
        if let Ok(panel) = engine::core::registry::call_api(world, "weather.panel", json!({})) {
            let live = rows(&panel);
            let mut data = world.resource_mut::<UiData>();
            for (name, value) in &mut next {
                if let Some(r) = live.iter().find(|r| &r.name == name)
                    && (r.value - *value).abs() > 1e-6
                {
                    *value = r.value;
                    data.set(&path(name), r.value);
                }
            }
        }
    }
    world.resource_mut::<Panel>().shown = next;
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::core::time::TimeSource;

    fn app() -> App {
        let mut app = App::new();
        app.world.resource_mut::<EngineConfig>().headless = true;
        app.world
            .resource_mut::<Time>()
            .set_source(TimeSource::Manual {
                step_secs: 1.0 / 60.0,
            });
        app.add_plugins((
            engine::input::InputPlugin,
            engine_ui::UiPlugin,
            engine::world::WorldPlugin,
            engine_weather::WeatherPlugin,
        ));
        install(&mut app);
        app.update_n(2);
        app
    }

    #[test]
    fn a_moved_slider_holds_its_value_and_auto_hands_it_back() {
        let mut app = app();
        toggle(&mut app.world);
        app.update();
        assert!(screens::find_screen(&mut app.world, KEY).is_some());
        assert!(app.world.resource::<Panel>().open);
        // Untouched sliders send nothing.
        app.update_n(3);
        assert_eq!(
            app.world
                .resource::<engine_weather::Weather>()
                .tweaks
                .cloud_cover,
            None
        );
        // A slider moved by hand (what the Slider widget writes) is sent to the weather.
        app.world
            .resource_mut::<UiData>()
            .set(&path("cloud_cover"), 0.9);
        app.world
            .resource_mut::<UiData>()
            .set(&path("wind_from_deg"), 90.0);
        app.update();
        let w = app.world.resource::<engine_weather::Weather>();
        assert_eq!(w.tweaks.cloud_cover, Some(0.9));
        assert_eq!(w.wind_from_deg, 90.0);
        // Half a second on, the slider still shows the hand-set value.
        app.update_n(40);
        let shown = app.world.resource::<UiData>().get_f64(&path("cloud_cover"));
        assert!((shown.unwrap() - 0.9).abs() < 1e-4, "{shown:?}");
        // "Let the weather decide" clears it and reopens the panel.
        engine_ui::input::dispatch(&mut app.world, &format!("{KEY}.auto"));
        app.update_n(2);
        assert_eq!(
            app.world
                .resource::<engine_weather::Weather>()
                .tweaks
                .cloud_cover,
            None
        );
        assert!(app.world.resource::<Panel>().open);
        // F4's toggle closes it.
        toggle(&mut app.world);
        app.update_n(30);
        assert!(!app.world.resource::<Panel>().open);
    }

    /// A picture of the panel over the valley sky: `WEATHER_PANEL_SHOT=out.png cargo test -p
    /// showcase --bin showcase weather_panel -- --ignored` (needs a GPU or lavapipe).
    #[test]
    #[ignore]
    fn shoot_the_panel() {
        let Ok(out) = std::env::var("WEATHER_PANEL_SHOT") else {
            return;
        };
        let mut app = App::new();
        app.world.resource_mut::<EngineConfig>().headless = true;
        {
            let mut w = app.world.resource_mut::<Window>();
            w.width = 1600;
            w.height = 900;
        }
        app.add_plugins(DefaultPlugins)
            .add_plugins((engine_ui::UiPlugin, engine_weather::WeatherPlugin));
        install(&mut app);
        app.world.spawn((
            Transform::default(),
            MeshRenderer::new(
                Shape::Plane {
                    size: [4000.0, 4000.0],
                    subdivisions: 1,
                },
                StandardMaterial::hex(0x4f7a3a).with_roughness(0.9),
            ),
        ));
        app.world.spawn((
            Camera::perspective(60.0),
            Transform::from_xyz(0.0, 1.7, 0.0).looking_at(DVec3::new(0.0, 300.0, -1000.0), Vec3::Y),
        ));
        engine::core::registry::call_api(
            &mut app.world,
            "weather.set",
            json!({"preset": "partly_cloudy", "transition_s": 0}),
        )
        .unwrap();
        engine::core::registry::call_api(&mut app.world, "weather.time", json!({"hour": 15.0}))
            .unwrap();
        app.update_n(10);
        toggle(&mut app.world);
        app.update_n(40);
        engine::render::capture_now(&mut app.world, std::path::Path::new(&out)).expect("shot");
    }
}
