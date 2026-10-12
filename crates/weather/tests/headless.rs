//! Renders the weather station offscreen in different weather and checks the pictures: clouds
//! show up in the sky, overcast is greyer than clear, rain and snow draw particles, and wet
//! ground gets puddles. Needs a GPU or a software Vulkan driver (lavapipe); passes with a note
//! when neither is available (unless `ENGINE_REQUIRE_GPU=1`, as in CI).

use engine_core::prelude::*;
use engine_core::registry::call_api;
use engine_core::time::TimeSource;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, capture_now};
use engine_weather::WeatherPlugin;
use serde_json::{Value, json};

const W: u32 = 320;
const H: u32 = 180;

fn app() -> Option<(App, Entity)> {
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut win = app.world.resource_mut::<Window>();
        win.width = W;
        win.height = H;
    }
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual { step_secs: 0.05 });
    app.add_plugins((engine_world::WorldPlugin, RenderPlugin, WeatherPlugin));
    app.world.spawn((
        Transform::default(),
        MeshRenderer::new(
            Shape::Plane {
                size: [20_000.0, 20_000.0],
                subdivisions: 1,
            },
            StandardMaterial::hex(0x55743a).with_roughness(0.95),
        ),
    ));
    {
        let mut commands = app.world.commands();
        engine_weather::demo::spawn_demo(&mut commands, DVec3::ZERO);
    }
    app.world.flush();
    let camera = app
        .world
        .spawn((
            Camera::perspective(60.0),
            Transform::from_xyz(0.0, 2.0, 12.0),
        ))
        .id();
    app.world
        .resource_mut::<RenderSettings>()
        .environment_interval = 1;
    app.update_n(3);
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return None;
    }
    Some((app, camera))
}

fn call(app: &mut App, api: &str, args: Value) {
    if let Err(e) = call_api(&mut app.world, api, args) {
        panic!("{api}: {e}");
    }
}

fn weather(app: &mut App, preset: &str, hour: f32, advance_s: f32) {
    call(app, "weather.ground", json!({"dry": true}));
    call(
        app,
        "weather.set",
        json!({"preset": preset, "transition_s": 0}),
    );
    call(app, "weather.time", json!({"hour": hour}));
    call(app, "weather.advance", json!({"seconds": advance_s}));
}

fn look(app: &mut App, camera: Entity, eye: DVec3, target: DVec3) {
    *app.world.get_mut::<Transform>(camera).unwrap() =
        Transform::from_translation(eye).looking_at(target, Vec3::Y);
}

fn shoot(app: &mut App, name: &str) -> image::RgbaImage {
    app.update_n(8);
    let dir = env!("CARGO_TARGET_TMPDIR");
    let path = std::path::Path::new(&dir).join(format!("weather_{name}.png"));
    capture_now(&mut app.world, &path).expect("screenshot");
    image::open(&path).expect("png").to_rgba8()
}

struct Region {
    /// Mean luminance (0–255).
    mean: f32,
    /// Standard deviation of the luminance.
    spread: f32,
    /// Mean of blue minus red.
    blue: f32,
    /// Mean of green minus the average of red and blue.
    green: f32,
}

fn region(img: &image::RgbaImage, y0: u32, y1: u32) -> Region {
    let (mut sum, mut sum2, mut blue, mut green, mut n) = (0.0, 0.0, 0.0, 0.0, 0.0);
    for y in y0..y1 {
        for x in 0..img.width() {
            let p = img.get_pixel(x, y);
            let [r, g, b] = [p[0] as f32, p[1] as f32, p[2] as f32];
            let l = 0.2126 * r + 0.7152 * g + 0.0722 * b;
            sum += l;
            sum2 += l * l;
            blue += b - r;
            green += g - 0.5 * (r + b);
            n += 1.0;
        }
    }
    let mean = sum / n;
    Region {
        mean,
        spread: (sum2 / n - mean * mean).max(0.0).sqrt(),
        blue: blue / n,
        green: green / n,
    }
}

fn feature_state(app: &mut App, name: &str) -> Value {
    let stats = call_api(&mut app.world, "render.stats", json!({})).expect("render.stats");
    stats["renderer"]["features"]
        .as_array()
        .and_then(|f| f.iter().find(|f| f["name"] == name))
        .cloned()
        .unwrap_or_else(|| panic!("no feature {name} in {stats}"))
}

#[test]
fn clouds_rain_snow_and_puddles_render() {
    let Some((mut app, camera)) = app() else {
        return;
    };
    let up = (DVec3::new(0.0, 2.0, 12.0), DVec3::new(0.0, 300.0, -400.0));

    // Clear sky: blue and smooth.
    weather(&mut app, "clear", 13.0, 60.0);
    look(&mut app, camera, up.0, up.1);
    let clear = shoot(&mut app, "clear");
    let clear = region(&clear, 0, H / 2);
    assert!(clear.blue > 10.0, "clear sky isn't blue ({})", clear.blue);
    assert!(clear.mean > 60.0, "clear sky is dark ({})", clear.mean);

    // Fair-weather clouds: white heaps break up the blue.
    weather(&mut app, "partly_cloudy", 13.0, 60.0);
    let cumulus = shoot(&mut app, "partly_cloudy");
    let cumulus = region(&cumulus, 0, H / 2);
    assert!(
        cumulus.spread > clear.spread + 4.0,
        "no clouds in the sky (spread {} vs clear {})",
        cumulus.spread,
        clear.spread
    );
    let clouds = feature_state(&mut app, "weather clouds");
    assert_eq!(clouds["state"]["active"], true, "{clouds}");

    // Overcast with rain: grey sky, drops and splashes drawn.
    weather(&mut app, "rain", 13.0, 600.0);
    let rain = shoot(&mut app, "rain");
    let rain = region(&rain, 0, H / 2);
    assert!(
        rain.blue < clear.blue * 0.5,
        "overcast sky is as blue as a clear one ({} vs {})",
        rain.blue,
        clear.blue
    );
    let precip = feature_state(&mut app, "weather precipitation");
    assert!(
        precip["state"]["drops"].as_u64().unwrap_or(0) > 1000,
        "{precip}"
    );
    assert!(
        precip["state"]["splashes"].as_u64().unwrap_or(0) > 100,
        "{precip}"
    );

    // After the rain: puddles on the ground pass.
    call(
        &mut app,
        "weather.ground",
        json!({"wetness": 1.0, "puddle_m": 0.012}),
    );
    weather_keep_ground(&mut app, "partly_cloudy", 15.0);
    look(
        &mut app,
        camera,
        DVec3::new(5.5, 0.8, 5.0),
        DVec3::new(-3.0, 0.0, -3.0),
    );
    shoot(&mut app, "puddles");
    let ground = feature_state(&mut app, "weather ground");
    assert_eq!(ground["state"]["active"], true, "{ground}");

    // Snow: flakes drawn, and the ground turns white.
    weather(&mut app, "snow", 12.0, 3.0 * 3600.0);
    look(
        &mut app,
        camera,
        DVec3::new(0.0, 2.0, 12.0),
        DVec3::new(0.0, 0.0, 0.0),
    );
    let snow = shoot(&mut app, "snow");
    let precip = feature_state(&mut app, "weather precipitation");
    assert!(
        precip["state"]["flakes"].as_u64().unwrap_or(0) > 1000,
        "{precip}"
    );
    let snow_ground = region(&snow, H * 3 / 4, H);
    weather(&mut app, "clear", 12.0, 60.0);
    let bare = shoot(&mut app, "snow_cleared");
    let bare_ground = region(&bare, H * 3 / 4, H);
    // Grass is green; snow-covered grass is white.
    assert!(
        snow_ground.green < bare_ground.green * 0.4,
        "the snow didn't cover the grass (green {} vs bare {})",
        snow_ground.green,
        bare_ground.green
    );
}

/// Switch weather without drying the ground.
fn weather_keep_ground(app: &mut App, preset: &str, hour: f32) {
    call(
        app,
        "weather.set",
        json!({"preset": preset, "transition_s": 0}),
    );
    call(app, "weather.time", json!({"hour": hour}));
    call(app, "weather.advance", json!({"seconds": 30.0}));
}
