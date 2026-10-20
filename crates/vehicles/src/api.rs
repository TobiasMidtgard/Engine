//! `vehicles.*` API functions: spawn, drive and read vehicles from the AI bridge, the console
//! and scripts.

use engine_core::math::{DQuat, DVec3, Vec3};
use engine_core::prelude::*;
use engine_core::registry::resolve_entity;
use engine_world::VehicleControls;
use engine_world::physics::Velocity;
use serde_json::{Value, json};

use crate::aero::Aircraft;
use crate::autopilot::{Autopilot, AutopilotMode};
use crate::chassis::{Vehicle, VehicleTelemetry};
use crate::presets;
use crate::ship::{AssistMode, Spacecraft};

fn vec3_arg(args: &Value, key: &str) -> Result<Option<DVec3>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => {
            let a: [f64; 3] =
                serde_json::from_value(v.clone()).map_err(|e| format!("{key}: {e}"))?;
            Ok(Some(DVec3::from_array(a)))
        }
    }
}

fn vehicle_entity(world: &mut World, args: &Value) -> Result<Entity, String> {
    let selector = args
        .get("vehicle")
        .or_else(|| args.get("entity"))
        .ok_or("missing \"vehicle\" (an entity id or name)")?;
    let e = resolve_entity(world, selector)?;
    if world.get::<Vehicle>(e).is_none() {
        return Err(format!("entity {e} is not a vehicle"));
    }
    Ok(e)
}

fn presets_api(_world: &mut World, _args: Value) -> Result<Value, String> {
    Ok(Value::Array(
        presets::PRESETS
            .iter()
            .map(|(name, about)| json!({"name": name, "about": about}))
            .collect(),
    ))
}

fn spawn_api(world: &mut World, args: Value) -> Result<Value, String> {
    let preset = args
        .get("preset")
        .and_then(Value::as_str)
        .ok_or("missing \"preset\" (see vehicles.presets)")?
        .to_string();
    let at = vec3_arg(&args, "at")?.unwrap_or(DVec3::ZERO);
    let yaw = args.get("yaw_deg").and_then(Value::as_f64).unwrap_or(0.0);
    let on_ground = args
        .get("on_ground")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let lift = if on_ground {
        presets::ride_height(&preset)
    } else {
        0.0
    };
    let transform = Transform::from_translation(at + DVec3::Y * lift)
        .with_rotation(DQuat::from_rotation_y(-yaw.to_radians()).as_quat());
    let mode: Option<AutopilotMode> = match args.get("autopilot") {
        None | Some(Value::Null) => None,
        Some(v) => Some(serde_json::from_value(v.clone()).map_err(|e| format!("autopilot: {e}"))?),
    };
    let entity = {
        let mut commands = world.commands();
        let e = presets::spawn(&mut commands, &preset, transform)
            .ok_or_else(|| format!("unknown preset '{preset}' (see vehicles.presets)"))?;
        if let Some(name) = args.get("name").and_then(Value::as_str) {
            commands.entity(e).insert(Name::new(name.to_string()));
        }
        if let Some(mode) = mode {
            commands.entity(e).insert(Autopilot::new(mode));
        }
        e
    };
    world.flush();
    // Spawned with its gear up: stowed from the start, as for a ship already in flight.
    if let Some(down) = gear_arg(&args)? {
        let mut aircraft = world
            .get_mut::<Aircraft>(entity)
            .ok_or("gear: only an aircraft has retractable gear")?;
        aircraft.gear_down = down;
        aircraft.gear = if down { 1.0 } else { 0.0 };
    }
    Ok(json!({"id": entity.to_bits()}))
}

/// `"gear": "up"` or `"down"`: whether the landing gear is wanted out.
fn gear_arg(args: &Value) -> Result<Option<bool>, String> {
    match args.get("gear") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(g)) if g.eq_ignore_ascii_case("up") => Ok(Some(false)),
        Some(Value::String(g)) if g.eq_ignore_ascii_case("down") => Ok(Some(true)),
        Some(other) => Err(format!("gear: \"up\" or \"down\", not {other}")),
    }
}

fn place_api(world: &mut World, args: Value) -> Result<Value, String> {
    let e = vehicle_entity(world, &args)?;
    // Preset key from the vehicle's name ("sports car" → "sports_car") for its ride height.
    let kind = world
        .get::<Vehicle>(e)
        .map(|v| v.name.replace(' ', "_"))
        .unwrap_or_default();
    let at = vec3_arg(&args, "at")?.ok_or("missing \"at\"")?;
    let yaw = args.get("yaw_deg").and_then(Value::as_f64).unwrap_or(0.0);
    let on_ground = args
        .get("on_ground")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let lift = if on_ground {
        presets::ride_height(&kind)
    } else {
        0.0
    };
    let rotation = DQuat::from_rotation_y(-yaw.to_radians()).as_quat();
    let forward = rotation * Vec3::NEG_Z;
    let speed = args.get("speed").and_then(Value::as_f64).unwrap_or(0.0) as f32;
    let linear = match vec3_arg(&args, "velocity")? {
        Some(v) => v.as_vec3(),
        None => forward * speed,
    };
    let yaw_rate = args
        .get("yaw_rate_deg")
        .and_then(Value::as_f64)
        .unwrap_or(0.0) as f32;
    if let Some(mut t) = world.get_mut::<Transform>(e) {
        t.translation = at + DVec3::Y * lift;
        t.rotation = rotation;
    }
    world.entity_mut(e).insert(Velocity {
        linear,
        angular: Vec3::Y * yaw_rate.to_radians(),
    });
    if let Some(mut i) = world.get_mut::<TransformInterpolation>(e) {
        i.reset();
    }
    crate::wheels::set_rolling(world, e, linear.dot(forward));
    // A path follower looks for its place on the path again, and a pilot picks its phase
    // (and runway) afresh (a drift keeps its phase).
    if let Some(mut ap) = world.get_mut::<Autopilot>(e)
        && matches!(
            ap.mode,
            AutopilotMode::Drive { .. } | AutopilotMode::Fly { .. }
        )
    {
        *ap = Autopilot::new(ap.mode.clone());
    }
    Ok(json!({"id": e.to_bits()}))
}

fn list_api(world: &mut World, _args: Value) -> Result<Value, String> {
    let mut q = world.query::<(
        Entity,
        &Vehicle,
        &Transform,
        &VehicleTelemetry,
        Option<&Name>,
        Option<&Autopilot>,
    )>();
    let out: Vec<Value> = q
        .iter(world)
        .map(|(e, v, t, tel, name, ap)| {
            json!({
                "id": e.to_bits(),
                "name": name.map(|n| n.as_str().to_string()).unwrap_or_else(|| v.name.clone()),
                "kind": v.kind,
                "position": t.translation.to_array(),
                "speed_kmh": tel.speed * 3.6,
                "altitude_m": tel.altitude_m,
                "autopilot": ap.map(|a| a.phase.clone()),
            })
        })
        .collect();
    Ok(Value::Array(out))
}

fn control_api(world: &mut World, args: Value) -> Result<Value, String> {
    let e = vehicle_entity(world, &args)?;
    let mut controls = world.get::<VehicleControls>(e).copied().unwrap_or_default();
    let mut v = serde_json::to_value(controls).map_err(|e| e.to_string())?;
    if let (Value::Object(dst), Value::Object(src)) = (&mut v, &args) {
        for (k, val) in src {
            if dst.contains_key(k) {
                dst.insert(k.clone(), val.clone());
            }
        }
    }
    controls = serde_json::from_value(v).map_err(|e| e.to_string())?;
    let assist = match args.get("assist") {
        None | Some(Value::Null) => None,
        Some(v) => Some(
            serde_json::from_value::<AssistMode>(v.clone()).map_err(|e| format!("assist: {e}"))?,
        ),
    };
    if let Some(down) = gear_arg(&args)? {
        let mut aircraft = world
            .get_mut::<Aircraft>(e)
            .ok_or("gear: only an aircraft has retractable gear")?;
        aircraft.gear_down = down;
    }
    if let Some(mode) = assist {
        let mut ship = world
            .get_mut::<Spacecraft>(e)
            .ok_or("assist: only a spacecraft has a flight assist")?;
        ship.assist.mode = mode;
    }
    // A human (or AI) at the wheel: the autopilot lets go.
    if let Some(mut ap) = world.get_mut::<Autopilot>(e) {
        ap.mode = AutopilotMode::Off;
        ap.phase.clear();
    }
    world.entity_mut(e).insert(controls);
    let mut out = serde_json::to_value(controls).map_err(|e| e.to_string())?;
    if let Value::Object(out) = &mut out {
        if let Some(ship) = world.get::<Spacecraft>(e) {
            out.insert("assist".into(), json!(ship.assist.mode));
        }
        if let Some(aircraft) = world.get::<Aircraft>(e) {
            out.insert(
                "gear".into(),
                json!(if aircraft.gear_down { "down" } else { "up" }),
            );
        }
    }
    Ok(out)
}

fn autopilot_api(world: &mut World, args: Value) -> Result<Value, String> {
    let e = vehicle_entity(world, &args)?;
    let mode: AutopilotMode = match args.get("mode") {
        None | Some(Value::Null) => AutopilotMode::Off,
        Some(Value::String(s)) if s.eq_ignore_ascii_case("off") => AutopilotMode::Off,
        Some(v) => serde_json::from_value(v.clone()).map_err(|e| format!("mode: {e}"))?,
    };
    world.entity_mut(e).insert(Autopilot::new(mode));
    if world
        .get::<Autopilot>(e)
        .is_some_and(|a| a.mode == AutopilotMode::Off)
    {
        world.entity_mut(e).insert(VehicleControls::default());
    }
    serde_json::to_value(world.get::<Autopilot>(e)).map_err(|e| e.to_string())
}

fn telemetry_api(world: &mut World, args: Value) -> Result<Value, String> {
    let e = vehicle_entity(world, &args)?;
    let tel = world
        .get::<VehicleTelemetry>(e)
        .copied()
        .unwrap_or_default();
    let mut v = serde_json::to_value(tel).map_err(|e| e.to_string())?;
    if let Value::Object(m) = &mut v {
        m.insert("speed_kmh".into(), json!(tel.speed * 3.6));
        if let Some(ap) = world.get::<Autopilot>(e) {
            m.insert("autopilot".into(), json!(ap.phase));
        }
    }
    Ok(v)
}

pub(crate) fn register(app: &mut App) {
    app.register_api(
        "vehicles.presets",
        "List the vehicle presets vehicles.spawn can make.",
        "{}",
        presets_api,
    )
    .register_api(
        "vehicles.spawn",
        "Spawn a vehicle preset at a point (world meters; resting on the ground there unless \
         on_ground is false), facing yaw_deg (0 = −Z, 90 = +X), optionally named, with an \
         autopilot and with its gear \"up\" (stowed) or \"down\". Returns its id.",
        r#"{"preset": "sports_car", "at": [0, 0, 0], "yaw_deg": 90, "name": "Red car", "autopilot": {"Drive": {"path": [[0,0,0],[100,0,0],[100,0,100]], "looped": true, "max_speed": 30, "lateral_g": 0.8}}}"#,
        spawn_api,
    )
    .register_api(
        "vehicles.place",
        "Move a vehicle to a point (world meters; on the ground there unless on_ground is \
         false), facing yaw_deg (0 = −Z, 90 = +X), moving at speed (m/s, along the nose) or \
         velocity ([x, y, z] m/s) and turning left at yaw_rate_deg (°/s), with its wheels \
         rolling and its gearbox in the matching gear. A driving autopilot finds its place on \
         its path again.",
        r#"{"vehicle": "Red car", "at": [120, 0, -40], "yaw_deg": 150, "speed": 30}"#,
        place_api,
    )
    .register_api(
        "vehicles.list",
        "Every vehicle: id, name, kind, position, speed (km/h), altitude and autopilot phase.",
        "{}",
        list_api,
    )
    .register_api(
        "vehicles.control",
        "Drive a vehicle by hand: set any of throttle, brake (0..1), steer, pitch, yaw, lift, \
         strafe (−1..1), handbrake, boost, the landing gear (\"up\" or \"down\", folding over \
         6 s) and a spacecraft's flight assist (Off, Rotation, Full or Hover). Turns its \
         autopilot off.",
        r#"{"vehicle": "Red car", "throttle": 1.0, "steer": -0.3}"#,
        control_api,
    )
    .register_api(
        "vehicles.autopilot",
        "Give a vehicle an autopilot mode: Off, Drive, Drift, Fly, StallDemo, Ascend, Hold, \
         Hop (fields as in the Autopilot component).",
        r#"{"vehicle": "Plane", "mode": {"Fly": {"waypoints": [[0,300,0],[2000,300,0]], "looped": true, "cruise_speed": 60, "rotate_speed": 33}}}"#,
        autopilot_api,
    )
    .register_api(
        "vehicles.telemetry",
        "A vehicle's gauges: speed, gear, rpm, airspeed, angle of attack, g-load, altitude, \
         orbit, hull temperature, what the autopilot is doing.",
        r#"{"vehicle": "Plane"}"#,
        telemetry_api,
    );
}
