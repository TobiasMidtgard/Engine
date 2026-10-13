//! `softbody.*` API functions: what AI agents, the console and scripts call.

use engine_core::prelude::*;
use engine_core::registry::resolve_entity;
use engine_world::{IgniteRequest, ProjectileTrace};
use serde_json::{Value, json};

use crate::body::{SoftBodySim, SoftKind};
use crate::rope::Lines;

fn vec3_arg(args: &Value, key: &str) -> Result<DVec3, String> {
    let v = args
        .get(key)
        .and_then(|v| v.as_array())
        .ok_or_else(|| format!("Give \"{key}\" as [x, y, z] in world meters."))?;
    if v.len() != 3 {
        return Err(format!("\"{key}\" needs three numbers: [x, y, z]."));
    }
    let n = |i: usize| {
        v[i].as_f64()
            .ok_or_else(|| format!("\"{key}\" must hold numbers."))
    };
    Ok(DVec3::new(n(0)?, n(1)?, n(2)?))
}

fn f32_arg(args: &Value, key: &str, default: f32) -> f32 {
    args.get(key)
        .and_then(|v| v.as_f64())
        .map(|v| v as f32)
        .unwrap_or(default)
}

fn bodies_near(world: &mut World, at: DVec3, radius: f32) -> Vec<Entity> {
    let mut q = world.query::<(Entity, &SoftBodySim)>();
    q.iter(world)
        .filter(|(_, b)| (b.center - at).length() < (b.radius + radius) as f64)
        .map(|(e, _)| e)
        .collect()
}

/// Cut ropes and tear cloth within `radius` of `at` in one body. Returns pieces removed.
pub fn cut(body: &mut SoftBodySim, at: DVec3, radius: f32) -> usize {
    let p = body.local(at);
    let b = body;
    let mut removed = 0;
    if let (Some(c), Some(s)) = (b.cloth.as_mut(), b.surface.as_mut()) {
        let doomed: Vec<usize> = (0..c.triangles.len())
            .filter(|&t| {
                c.alive[t]
                    && c.triangles[t]
                        .iter()
                        .any(|&i| b.sim.pos[i as usize].distance(p) < radius)
            })
            .collect();
        removed += doomed.len();
        if c.kill(doomed, &mut b.sim, s) {
            b.topology_version += 1;
        }
    }
    if let Some(state) = b.bridge.as_mut() {
        let lines: &mut Lines = &mut state.lines;
        let hit = lines.segments_near(&b.sim, p, p, radius);
        removed += hit.len();
        if lines.cut(&mut b.sim, &hit) {
            b.tubes = lines.tubes();
            b.topology_version += 1;
        }
    }
    removed
}

pub fn register(app: &mut App) {
    app.register_api(
        "softbody.list",
        "Every cloth, rope, bridge, soft volume and hair groom: kind, particles, constraints, pins, center, step time.",
        "{}",
        |world, _| {
            let mut q = world.query::<(Entity, &SoftBodySim, Option<&Name>)>();
            Ok(Value::Array(
                q.iter(world)
                    .map(|(e, b, name)| {
                        let mut d = b.describe();
                        d["entity"] = json!(e.to_bits());
                        d["name"] = json!(name.map(|n| n.as_str().to_string()));
                        d
                    })
                    .collect(),
            ))
        },
    )
    .register_api(
        "softbody.cut",
        "Cut ropes and tear cloth within radius (m) of a world point. Returns how many pieces were removed.",
        r#"{"at": [110, 3, -10], "radius": 0.3}"#,
        |world, args| {
            let at = vec3_arg(&args, "at")?;
            let radius = f32_arg(&args, "radius", 0.3).max(0.01);
            let mut removed = 0;
            for e in bodies_near(world, at, radius) {
                if let Some(mut b) = world.get_mut::<SoftBodySim>(e) {
                    removed += cut(&mut b, at, radius);
                }
            }
            Ok(json!({ "removed": removed }))
        },
    )
    .register_api(
        "softbody.shoot",
        "Fire a projectile from one world point to another (a ProjectileTrace every module reacts to: cloth gets a hole, ropes are cut, smoke parts).",
        r#"{"from": [100, 3, -20], "to": [120, 3, -20], "radius": 0.02, "speed": 400}"#,
        |world, args| {
            let from = vec3_arg(&args, "from")?;
            let to = vec3_arg(&args, "to")?;
            world.write_message(ProjectileTrace {
                from,
                to,
                radius: f32_arg(&args, "radius", 0.02),
                speed: f32_arg(&args, "speed", 400.0),
                shooter: None,
            });
            Ok(json!({ "fired": true }))
        },
    )
    .register_api(
        "softbody.ignite",
        "Set flammable cloth on fire within radius (m) of a world point (also sends an IgniteRequest to the fire module).",
        r#"{"at": [98, 0.5, -14], "radius": 0.3}"#,
        |world, args| {
            let at = vec3_arg(&args, "at")?;
            let radius = f32_arg(&args, "radius", 0.3).max(0.01);
            let mut lit = 0;
            for e in bodies_near(world, at, radius) {
                if let Some(mut b) = world.get_mut::<SoftBodySim>(e) {
                    let p = b.local(at);
                    let b = &mut *b;
                    if let Some(c) = b.cloth.as_mut() {
                        lit += c.ignite(&b.sim, p, radius);
                    }
                }
            }
            world.write_message(IgniteRequest {
                position: at,
                radius,
                temperature_c: 600.0,
                source: None,
            });
            Ok(json!({ "triangles_lit": lit }))
        },
    )
    .register_api(
        "softbody.push",
        "Change the velocity (m/s) of every soft particle within radius (m) of a world point.",
        r#"{"at": [110, 2, -4], "radius": 1.5, "velocity": [0, 3, -6]}"#,
        |world, args| {
            let at = vec3_arg(&args, "at")?;
            let radius = f32_arg(&args, "radius", 1.0).max(0.01);
            let v = vec3_arg(&args, "velocity")?.as_vec3();
            let mut moved = 0;
            for e in bodies_near(world, at, radius) {
                if let Some(mut b) = world.get_mut::<SoftBodySim>(e) {
                    let c = b.local(at);
                    b.sim.add_velocity(|p| {
                        if p.distance(c) < radius {
                            moved += 1;
                            v
                        } else {
                            Vec3::ZERO
                        }
                    });
                }
            }
            Ok(json!({ "particles": moved }))
        },
    )
    .register_api(
        "softbody.unpin",
        "Release pins: all pins of one body ({\"entity\": id or name}), or pins within radius (m) of a world point ({\"at\": [x,y,z]}). Drops flags and curtains.",
        r#"{"at": [98, 6, -14], "radius": 1.0}"#,
        |world, args| {
            let targets: Vec<(Entity, Option<(DVec3, f32)>)> = if let Some(sel) = args.get("entity") {
                vec![(resolve_entity(world, sel)?, None)]
            } else {
                let at = vec3_arg(&args, "at")?;
                let radius = f32_arg(&args, "radius", 1.0).max(0.01);
                bodies_near(world, at, radius)
                    .into_iter()
                    .map(|e| (e, Some((at, radius))))
                    .collect()
            };
            let mut released = 0;
            for (e, near) in targets {
                let Some(mut b) = world.get_mut::<SoftBodySim>(e) else {
                    continue;
                };
                let mut pins: Vec<u32> = b.sim.pins.iter().map(|p| p.p).collect();
                if b.kind == SoftKind::Cloth {
                    // Cloth on a moving body is held by stiff goals instead of pins.
                    pins.extend(b.sim.goals.iter().map(|g| g.p));
                    pins.sort_unstable();
                    pins.dedup();
                }
                for p in pins {
                    let close = near.is_none_or(|(at, r)| {
                        (b.world_position(p as usize) - at).length() < r as f64
                    });
                    if close {
                        b.sim.unpin(p);
                        released += 1;
                    }
                }
                b.sim.tethers.clear();
            }
            Ok(json!({ "released": released }))
        },
    )
    .register_api(
        "softbody.reset",
        "Rebuild a soft body from its spec (undo tears, cuts and burns).",
        r#"{"entity": "Curtain"}"#,
        |world, args| {
            let e = resolve_entity(world, args.get("entity").ok_or("Give \"entity\": id or name.")?)?;
            let planks: Vec<Entity> = world
                .get::<SoftBodySim>(e)
                .ok_or("That entity has no soft body.")?
                .bridge
                .as_ref()
                .map(|b| b.planks.iter().map(|p| p.0).collect())
                .unwrap_or_default();
            for p in planks {
                world.despawn(p);
            }
            world.entity_mut(e).remove::<SoftBodySim>();
            Ok(json!({ "reset": e.to_bits() }))
        },
    )
    .register_api(
        "softbody.stats",
        "Soft-body totals: bodies, particles, frozen bodies, last step time (ms).",
        "{}",
        |world, _| {
            let d = world.resource::<Diagnostics>();
            Ok(json!({
                "bodies": d.get("softbody.bodies"),
                "particles": d.get("softbody.particles"),
                "frozen": d.get("softbody.frozen"),
                "step_ms": d.get("softbody.step_ms"),
            }))
        },
    );
}
