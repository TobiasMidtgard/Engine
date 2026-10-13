//! `water.*` functions for the AI bridge, console and scripts.

use engine_core::math::{DVec2, DVec3, EulerRot, Vec3};
use engine_core::prelude::*;
use serde_json::{Value, json};

use crate::body::{WaterBed, WaterBody, WaterOptics};
use crate::flow::{FlowRegion, FlowSource};
use crate::interact::splash;
use crate::state::SharedWater;

fn vec3_arg(args: &Value, key: &str) -> Result<DVec3, String> {
    let a = args
        .get(key)
        .and_then(|v| v.as_array())
        .ok_or_else(|| format!("Give \"{key}\" as [x, y, z] in world meters."))?;
    let n = |i: usize| a.get(i).and_then(|v| v.as_f64());
    match (n(0), n(1), n(2)) {
        (Some(x), Some(y), Some(z)) => Ok(DVec3::new(x, y, z)),
        _ => Err(format!("\"{key}\" needs three numbers: [x, y, z].")),
    }
}

fn f32_arg(args: &Value, key: &str, default: f32) -> f32 {
    args.get(key)
        .and_then(|v| v.as_f64())
        .map(|v| v as f32)
        .unwrap_or(default)
}

fn v3(v: Vec3) -> Value {
    json!([v.x, v.y, v.z])
}

pub fn register(app: &mut App) {
    app.register_api(
        "water.sample",
        "Water at a point: surface height (with waves and ripples), depth, current, normal, and \
         how far under the surface the point is (negative = above).",
        r#"{"at": [170, 4, -140]}"#,
        |world, args| {
            let at = vec3_arg(&args, "at")?;
            let shared = world.resource::<SharedWater>().clone();
            let state = shared.0.read();
            Ok(match state.sample(at) {
                Some(s) => json!({
                    "water": true,
                    "surface_height": s.surface_height,
                    "depth": s.depth,
                    "velocity": v3(s.velocity),
                    "normal": v3(s.normal),
                    "submersion": s.surface_height - at.y,
                }),
                None => json!({ "water": false }),
            })
        },
    )
    .register_api(
        "water.splash",
        "Disturb the surface: a ring of waves and foam (a stone, a cannonball, a whale). \
         `radius` in meters, `strength` 0.1 pebble .. 1 person jumping in .. 5 depth charge.",
        r#"{"at": [170, 4, -140], "radius": 0.8, "strength": 1.0}"#,
        |world, args| {
            let at = vec3_arg(&args, "at")?;
            let radius = f32_arg(&args, "radius", 0.5).max(0.05);
            let strength = f32_arg(&args, "strength", 1.0).max(0.0);
            let shared = world.resource::<SharedWater>().clone();
            let mut state = shared.0.write();
            if state.sample(at).is_none() {
                return Err("There is no water at that point. Use water.bodies to find some.".into());
            }
            let mut d = splash(at, radius, 6.0 * strength);
            d.depth *= strength.max(0.1);
            state.ripples.disturb(d);
            Ok(json!({ "ok": true }))
        },
    )
    .register_api(
        "water.bodies",
        "List water bodies and flowing regions: entity, center, level, extent, current wave \
         height (m) and the wind driving them.",
        "{}",
        |world, _| {
            let shared = world.resource::<SharedWater>().clone();
            let state = shared.0.read();
            let bodies: Vec<Value> = state
                .bodies
                .iter()
                .map(|b| {
                    json!({
                        "entity": b.entity.to_bits(),
                        "center": [b.center.x, b.center.y, b.center.z],
                        "level": b.center.y,
                        "extent": format!("{:?}", b.body.extent),
                        "bed": format!("{:?}", b.body.bed),
                        "wave_height_m": b.waves.significant_height(),
                        "wind_m_s": v3(b.wind),
                        "interactive": b.body.interactive,
                    })
                })
                .collect();
            let flows: Vec<Value> = state
                .flows
                .iter()
                .map(|f| {
                    json!({
                        "entity": f.entity.to_bits(),
                        "origin": [f.origin.x, f.origin.y, f.origin.z],
                        "cells": [f.nx, f.nz],
                        "cell_m": f.cell,
                        "volume_m3": f.volume(),
                        "sources": f.sources.len(),
                    })
                })
                .collect();
            Ok(json!({ "bodies": bodies, "flows": flows }))
        },
    )
    .register_api(
        "water.spawn_lake",
        "Create a round lake (or pond) whose surface is at `at`. `optics` is a look preset: \
         clear_lake, ocean, tropical, murky_pond, stream. Uses the terrain as its bed unless \
         `depth` is given (then a smooth bowl that deep).",
        r#"{"at": [0, 2, 0], "radius": 15, "depth": 3, "optics": "clear_lake"}"#,
        |world, args| {
            let at = vec3_arg(&args, "at")?;
            let radius = f32_arg(&args, "radius", 15.0).max(0.5);
            let mut body = WaterBody::lake(radius);
            if let Some(d) = args.get("depth").and_then(|v| v.as_f64()) {
                body.bed = WaterBed::Bowl {
                    depth_m: d as f32,
                    shore_m: (radius * 0.5).min(d as f32 * 6.0),
                };
            }
            if let Some(name) = args.get("optics").and_then(|v| v.as_str()) {
                body.optics = WaterOptics::preset(name).ok_or_else(|| {
                    format!(
                        "Unknown optics \"{name}\". Use clear_lake, ocean, tropical, murky_pond or stream."
                    )
                })?;
            }
            let e = world
                .spawn((Name::new("Lake"), Transform::from_translation(at), body))
                .id();
            Ok(json!({ "entity": e.to_bits() }))
        },
    )
    .register_api(
        "water.spawn_ocean",
        "Create an endless ocean at sea level `level` (world y) with wind waves and swell.",
        r#"{"level": 0.0}"#,
        |world, args| {
            let level = f32_arg(&args, "level", 0.0) as f64;
            let e = world
                .spawn((
                    Name::new("Ocean"),
                    Transform::from_xyz(0.0, level, 0.0),
                    WaterBody::ocean(),
                ))
                .id();
            Ok(json!({ "entity": e.to_bits() }))
        },
    )
    .register_api(
        "water.pour",
        "Add a water source inside a flowing-water region (a spring, a burst pipe, a magic \
         fountain). `rate_m3_s`: 0.01 tap, 0.2 stream, 5 flood.",
        r#"{"at": [100, 6, -150], "rate_m3_s": 0.3, "radius": 1.0}"#,
        |world, args| {
            let at = vec3_arg(&args, "at")?;
            let rate = f32_arg(&args, "rate_m3_s", 0.2);
            let radius = f32_arg(&args, "radius", 1.0).max(0.1);
            let mut q = world.query::<(Entity, &GlobalTransform, &mut FlowRegion)>();
            for (e, gt, mut region) in q.iter_mut(world) {
                let yaw = gt.rotation().to_euler(EulerRot::YXZ).0;
                let local = crate::flow::grid_local(gt.translation(), yaw, at);
                let size = DVec2::new(region.cells[0] as f64, region.cells[1] as f64)
                    * region.cell_m as f64;
                if local.x >= 0.0 && local.y >= 0.0 && local.x < size.x && local.y < size.y {
                    region.sources.push(FlowSource {
                        at: [local.x as f32, local.y as f32],
                        radius_m: radius,
                        rate_m3_s: rate,
                    });
                    return Ok(json!({ "region": e.to_bits(), "sources": region.sources.len() }));
                }
            }
            Err("No flowing-water region covers that point. Add a FlowRegion entity there first."
                .into())
        },
    )
    .register_api(
        "water.calm",
        "Remove every ripple, wake and splash (the surface keeps its wind waves).",
        "{}",
        |world, _| {
            let shared = world.resource::<SharedWater>().clone();
            let mut state = shared.0.write();
            let r = &mut state.ripples;
            let (n, cell, origin) = (r.n, r.cell, r.origin);
            let speeds = r.speed2.clone();
            let has = r.has_water;
            *r = crate::ripples::RippleField::new(n, cell);
            r.origin = origin;
            r.speed2 = speeds;
            r.has_water = has;
            Ok(json!({ "ok": true }))
        },
    );
}
