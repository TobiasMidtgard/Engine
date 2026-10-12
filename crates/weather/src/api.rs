//! `weather.*` API functions: what the console, scripts and AI agents call.

use engine_core::math::DVec3;
use engine_core::prelude::*;
use engine_world::{FocusPoints, HeatField, SurfaceLayers, WeatherState};
use serde_json::{Value, json};

use crate::climate::Climate;
use crate::lightning::StrikeRequests;
use crate::presets::WeatherPreset;
use crate::sim::{self, ForecastStep, Weather, WeatherSim};
use crate::sky::{Sky, SkyLights};
use crate::surface::{self, GroundCache, GroundWeather};

fn preset_arg(v: &Value) -> Result<WeatherPreset, String> {
    let name = v.as_str().unwrap_or_default();
    WeatherPreset::from_name(name).ok_or_else(|| {
        let names: Vec<_> = WeatherPreset::ALL.iter().map(|p| p.name()).collect();
        format!(
            "Unknown weather \"{name}\". Use one of: {}.",
            names.join(", ")
        )
    })
}

fn f32_arg(args: &Value, key: &str) -> Option<f32> {
    args.get(key).and_then(Value::as_f64).map(|v| v as f32)
}

/// A summary of the weather right now.
pub fn describe(world: &World) -> Value {
    let state = world.resource::<WeatherState>();
    let sim = world.resource::<WeatherSim>();
    let weather = world.resource::<Weather>();
    let lights = world.resource::<SkyLights>();
    let focus = sim.focus;
    json!({
        "here": sim.preset_at(focus).name(),
        "target": weather.preset.name(),
        "auto": weather.auto,
        "fronts": sim.fronts.iter().map(|f| json!({
            "preset": f.preset.name(),
            "arrived_here": sim.front_weight(f, focus),
        })).collect::<Vec<_>>(),
        "forecast": weather.forecast.iter().map(|s| json!({
            "preset": s.preset.name(), "after_s": s.after_s, "transition_s": s.transition_s,
        })).collect::<Vec<_>>(),
        "temperature_c": state.temperature_c,
        "humidity": state.humidity,
        "pressure_hpa": state.pressure_hpa,
        "cloud_cover": state.cloud_cover,
        "cloud_base_m": state.cloud_base_m,
        "cloud_top_m": state.cloud_top_m,
        "precipitation_mm_h": state.precipitation_mm_h,
        "precipitation": format!("{:?}", state.precipitation_kind).to_lowercase(),
        "wind_m_s": state.wind.to_array(),
        "wind_speed_m_s": state.wind.length(),
        "gustiness": state.gustiness,
        "fog": state.fog,
        "storm": state.storm,
        "hour": state.time_of_day_h,
        "day_of_year": state.day_of_year,
        "sun_elevation_deg": lights.sun_elevation_deg,
        "sky_clock_drives_lights": lights.in_charge,
    })
}

/// Fast-forward the weather (fronts, air, ground) by `seconds` of game time.
pub fn advance(world: &mut World, seconds: f32, with_clock: bool) {
    let step_s = 5.0f32;
    let steps = (seconds / step_s).ceil().max(0.0) as usize;
    let focus = world.resource::<FocusPoints>().primary();
    for _ in 0..steps {
        if with_clock {
            let mut sky = world.resource_mut::<Sky>();
            if sky.hours_per_second != 0.0 {
                let hours = sky.hours_per_second * step_s;
                sky.hour = (sky.hour + hours).rem_euclid(24.0);
            }
        }
        // The sky clock's time when it drives the sun, otherwise the game's.
        let clock_driven = world
            .query_filtered::<(), With<crate::sky::SunLight>>()
            .iter(world)
            .next()
            .is_some();
        let (hour, day) = if clock_driven {
            let sky = *world.resource::<Sky>();
            let mut state = world.resource_mut::<WeatherState>();
            state.time_of_day_h = sky.hour;
            state.day_of_year = sky.day_of_year;
            (sky.hour, sky.day_of_year)
        } else {
            let state = world.resource::<WeatherState>();
            (state.time_of_day_h, state.day_of_year)
        };
        let climate = *world.resource::<Climate>();
        let mut weather = world.resource::<Weather>().clone();
        world.resource_scope(|world, mut sim: Mut<WeatherSim>| {
            sim::step(&mut sim, &mut weather, &climate, focus, hour, day, step_s);
            sim::write_state(&sim, &mut world.resource_mut::<WeatherState>());
        });
        *world.resource_mut::<Weather>() = weather;
        let params = *world.resource::<GroundWeather>();
        let state = world.resource::<WeatherState>().clone();
        world.resource_scope(|world, mut surface: Mut<SurfaceLayers>| {
            let mut cache = world.resource_mut::<GroundCache>();
            let cache = std::mem::take(&mut *cache);
            let heat = world.get_resource::<HeatField>();
            surface::update_cells(
                &mut surface,
                &cache,
                &params,
                &surface::GroundInputs {
                    weather: &state,
                    heat,
                    dt: step_s,
                },
            );
            *world.resource_mut::<GroundCache>() = cache;
        });
    }
}

pub(crate) fn register(app: &mut App) {
    app.register_api(
        "weather.set",
        "Change the weather. A front carries it in from upwind over transition_s game seconds \
         (0 = at once). Presets: clear, partly_cloudy, overcast, showers, rain, storm, snow, blizzard, fog.",
        r#"{"preset": "storm", "transition_s": 300, "auto": false, "wind_from_deg": 250}"#,
        |world, args| {
            let preset = preset_arg(args.get("preset").unwrap_or(&Value::Null))?;
            let mut weather = world.resource_mut::<Weather>();
            if let Some(t) = f32_arg(&args, "transition_s") {
                weather.transition_s = t.max(0.0);
            }
            if let Some(a) = args.get("auto").and_then(Value::as_bool) {
                weather.auto = a;
            }
            if let Some(d) = f32_arg(&args, "wind_from_deg") {
                weather.wind_from_deg = d;
            }
            weather.preset = preset;
            weather.forecast.clear();
            Ok(json!({ "target": preset.name(), "arrives_in_s": weather.transition_s }))
        },
    )
    .register_api(
        "weather.get",
        "The weather now: what is overhead, what is coming, temperature, humidity, wind, \
         clouds, rain, the hour and the sun.",
        "{}",
        |world, _| Ok(describe(world)),
    )
    .register_api(
        "weather.presets",
        "List the weather presets and what each sets (cloud cover, rain mm/h, wind m/s…).",
        "{}",
        |_, _| {
            Ok(json!(
                WeatherPreset::ALL
                    .iter()
                    .map(|p| json!({ "name": p.name(), "regime": p.regime() }))
                    .collect::<Vec<_>>()
            ))
        },
    )
    .register_api(
        "weather.forecast",
        "Queue weather changes: each step starts after_s game seconds after the previous one \
         and takes transition_s to arrive. \"replace\": false appends to the queue.",
        r#"{"steps": [{"preset": "storm", "after_s": 0, "transition_s": 240}, {"preset": "clear", "after_s": 900, "transition_s": 600}], "replace": true}"#,
        |world, args| {
            let steps = args
                .get("steps")
                .and_then(Value::as_array)
                .ok_or("Pass \"steps\": a list of {preset, after_s, transition_s}.")?;
            let mut parsed = Vec::new();
            for s in steps {
                parsed.push(ForecastStep {
                    preset: preset_arg(s.get("preset").unwrap_or(&Value::Null))?,
                    after_s: f32_arg(s, "after_s").unwrap_or(0.0).max(0.0),
                    transition_s: f32_arg(s, "transition_s").unwrap_or(600.0).max(0.0),
                });
            }
            let replace = args.get("replace").and_then(Value::as_bool).unwrap_or(true);
            let time_s = world.resource::<WeatherSim>().time_s;
            let first_after = parsed.first().map(|s| s.after_s).unwrap_or(0.0);
            {
                let mut weather = world.resource_mut::<Weather>();
                if replace {
                    weather.forecast = parsed;
                } else {
                    weather.forecast.extend(parsed);
                }
            }
            if replace {
                world.resource_mut::<WeatherSim>().next_change_s = time_s + first_after as f64;
            }
            let queued = world.resource::<Weather>().forecast.len();
            Ok(json!({ "queued": queued }))
        },
    )
    .register_api(
        "weather.time",
        "Set the clock: hour (0–24, 12 = noon), day_of_year (80 ≈ spring equinox), \
         hours_per_second (0 stops time), moon_phase (0 new, 0.5 full).",
        r#"{"hour": 18.0, "day_of_year": 80, "hours_per_second": 0.0, "moon_phase": 0.5}"#,
        |world, args| {
            let mut sky = world.resource_mut::<Sky>();
            if let Some(h) = f32_arg(&args, "hour") {
                sky.hour = h.rem_euclid(24.0);
            }
            if let Some(d) = f32_arg(&args, "day_of_year") {
                sky.day_of_year = d.rem_euclid(366.0);
            }
            if let Some(s) = f32_arg(&args, "hours_per_second") {
                sky.hours_per_second = s;
            }
            if let Some(p) = f32_arg(&args, "moon_phase") {
                sky.moon_phase = p.rem_euclid(1.0);
            }
            let sky = *sky;
            let driven = world.resource::<SkyLights>().in_charge;
            Ok(json!({
                "hour": sky.hour, "day_of_year": sky.day_of_year,
                "hours_per_second": sky.hours_per_second, "moon_phase": sky.moon_phase,
                "drives_lights": driven,
            }))
        },
    )
    .register_api(
        "weather.advance",
        "Fast-forward the weather by game seconds: fronts move, air warms or cools, ground \
         gets wet or dries. with_clock also moves the sun if the clock is running.",
        r#"{"seconds": 600, "with_clock": false}"#,
        |world, args| {
            let seconds = f32_arg(&args, "seconds")
                .ok_or("Pass \"seconds\": how much game time to simulate.")?;
            if !(0.0..=7.0 * 86400.0).contains(&seconds) {
                return Err("\"seconds\" must be between 0 and a week (604800).".into());
            }
            let with_clock = args
                .get("with_clock")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            advance(world, seconds, with_clock);
            Ok(describe(world))
        },
    )
    .register_api(
        "weather.ground",
        "Set the ground cover everywhere around the player at once: wetness (0..1), \
         puddle_m (water depth in the dips), snow_m (snow depth). Omitted values are kept; \
         {\"dry\": true} clears everything.",
        r#"{"wetness": 1.0, "puddle_m": 0.01, "snow_m": 0.0}"#,
        |world, args| {
            let dry = args.get("dry").and_then(Value::as_bool).unwrap_or(false);
            let wetness = f32_arg(&args, "wetness").map(|v| v.clamp(0.0, 1.0));
            let puddle = f32_arg(&args, "puddle_m").map(|v| v.clamp(0.0, 1.0));
            let snow = f32_arg(&args, "snow_m").map(|v| v.clamp(0.0, 20.0));
            if !dry && wetness.is_none() && puddle.is_none() && snow.is_none() {
                return Err("Pass \"dry\": true, or any of wetness, puddle_m, snow_m.".into());
            }
            let params = *world.resource::<GroundWeather>();
            let (seed, frame) = {
                let sim = world.resource::<WeatherSim>();
                (sim.seed, sim.frame)
            };
            let mut surface = world.resource_mut::<SurfaceLayers>();
            let grid = &mut surface.grid;
            let (origin, cell, width) = (grid.origin, grid.cell as f64, grid.width as usize);
            for (i, c) in grid.data.iter_mut().enumerate() {
                if dry {
                    c.wetness = 0.0;
                    c.puddle_m = 0.0;
                    c.snow_m = 0.0;
                }
                if let Some(w) = wetness {
                    c.wetness = w;
                }
                if let Some(p) = puddle {
                    // Water stands in the dips only.
                    let xz = origin
                        + engine_core::math::DVec2::new(
                            (i % width) as f64 + 0.5,
                            (i / width) as f64 + 0.5,
                        ) * cell;
                    let dip = surface::puddle_dip(frame.to_pattern(xz), params.puddle_share, seed);
                    c.puddle_m = (p * dip).min(params.max_puddle_m);
                }
                if let Some(s) = snow {
                    c.snow_m = s;
                }
            }
            Ok(json!({ "cells": grid.data.len() }))
        },
    )
    .register_api(
        "weather.lightning",
        "Strike lightning at a world position (x, y, z); y is ignored, it hits the ground or \
         the tallest thing there.",
        r#"{"at": [120, 0, 140]}"#,
        |world, args| {
            let at = args
                .get("at")
                .and_then(Value::as_array)
                .filter(|a| a.len() == 3)
                .ok_or("Pass \"at\": [x, y, z] in world meters.")?;
            let v: Vec<f64> = at.iter().map(|x| x.as_f64().unwrap_or(0.0)).collect();
            world
                .resource_mut::<StrikeRequests>()
                .0
                .push(DVec3::new(v[0], v[1], v[2]));
            Ok(json!({ "queued": true }))
        },
    );
}
