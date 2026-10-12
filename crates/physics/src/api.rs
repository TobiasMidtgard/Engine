//! `physics.*` API functions for the AI bridge, console and scripts.

use engine_core::math::{DVec3, Quat, Vec3};
use engine_core::prelude::*;
use engine_core::registry::resolve_entity;
use engine_world::physics::{ApplyImpulse, ColliderShape, Hit, PhysicsQueries, QueryFilter};
use engine_world::{SurfaceMaterials, WorldImpulse};
use serde_json::{Value, json};

use crate::{PhysicsDebug, PhysicsStats};

fn parse<T: serde::de::DeserializeOwned>(args: Value) -> Result<T, String> {
    serde_json::from_value(args).map_err(|e| format!("bad arguments: {e}"))
}

fn default_max() -> f32 {
    1000.0
}

/// Filter fields shared by every query.
#[derive(Deserialize, Default)]
#[serde(default)]
struct FilterArgs {
    /// Only colliders in these layers (bit mask, 0 = all).
    layers: u32,
    /// Entities to ignore (ids or names).
    exclude: Vec<Value>,
    include_sensors: bool,
}

impl FilterArgs {
    fn build(self, world: &mut World) -> Result<QueryFilter, String> {
        let exclude = self
            .exclude
            .iter()
            .map(|v| resolve_entity(world, v))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(QueryFilter {
            layers: self.layers,
            exclude,
            include_sensors: self.include_sensors,
        })
    }
}

fn hit_json(world: &World, hit: Option<Hit>) -> Value {
    let Some(h) = hit else {
        return json!({ "hit": false });
    };
    json!({
        "hit": true,
        "entity": h.entity.to_bits(),
        "name": world.get::<Name>(h.entity).map(|n| n.as_str().to_string()),
        "point": [h.point.x, h.point.y, h.point.z],
        "normal": [h.normal.x, h.normal.y, h.normal.z],
        "distance": h.distance,
        "surface": world
            .get_resource::<SurfaceMaterials>()
            .map(|m| m.get(h.surface).name.clone()),
    })
}

#[derive(Deserialize)]
struct RayArgs {
    origin: DVec3,
    dir: Vec3,
    #[serde(default = "default_max")]
    max_distance: f32,
    #[serde(flatten)]
    filter: FilterArgs,
}

fn raycast(world: &mut World, args: Value) -> Result<Value, String> {
    let a: RayArgs = parse(args)?;
    let filter = a.filter.build(world)?;
    let q = world.resource::<PhysicsQueries>().clone();
    let hit = q.raycast(a.origin, a.dir, a.max_distance, &filter);
    Ok(hit_json(world, hit))
}

#[derive(Deserialize)]
struct CastArgs {
    shape: ColliderShape,
    origin: DVec3,
    #[serde(default)]
    rotation: Option<Quat>,
    dir: Vec3,
    #[serde(default = "default_max")]
    max_distance: f32,
    #[serde(flatten)]
    filter: FilterArgs,
}

fn shapecast(world: &mut World, args: Value) -> Result<Value, String> {
    let a: CastArgs = parse(args)?;
    let filter = a.filter.build(world)?;
    let q = world.resource::<PhysicsQueries>().clone();
    let hit = q.shapecast(
        &a.shape,
        a.origin,
        a.rotation.unwrap_or(Quat::IDENTITY),
        a.dir,
        a.max_distance,
        &filter,
    );
    Ok(hit_json(world, hit))
}

#[derive(Deserialize)]
struct OverlapArgs {
    shape: ColliderShape,
    position: DVec3,
    #[serde(default)]
    rotation: Option<Quat>,
    #[serde(flatten)]
    filter: FilterArgs,
}

fn overlap(world: &mut World, args: Value) -> Result<Value, String> {
    let a: OverlapArgs = parse(args)?;
    let filter = a.filter.build(world)?;
    let q = world.resource::<PhysicsQueries>().clone();
    let mut out = Vec::new();
    q.overlap(
        &a.shape,
        a.position,
        a.rotation.unwrap_or(Quat::IDENTITY),
        &filter,
        &mut out,
    );
    Ok(json!({
        "entities": out
            .iter()
            .map(|e| json!({
                "id": e.to_bits(),
                "name": world.get::<Name>(*e).map(|n| n.as_str().to_string()),
            }))
            .collect::<Vec<_>>(),
    }))
}

#[derive(Deserialize)]
struct ClosestArgs {
    point: DVec3,
    #[serde(default = "default_max")]
    max_distance: f32,
    #[serde(flatten)]
    filter: FilterArgs,
}

fn closest_point(world: &mut World, args: Value) -> Result<Value, String> {
    let a: ClosestArgs = parse(args)?;
    let filter = a.filter.build(world)?;
    let q = world.resource::<PhysicsQueries>().clone();
    let hit = q.closest_point(a.point, a.max_distance, &filter);
    Ok(hit_json(world, hit))
}

#[derive(Deserialize)]
struct ImpulseArgs {
    entity: Value,
    impulse: Vec3,
    #[serde(default)]
    at: Option<DVec3>,
    #[serde(default)]
    angular: Vec3,
}

fn impulse(world: &mut World, args: Value) -> Result<Value, String> {
    let a: ImpulseArgs = parse(args)?;
    let entity = resolve_entity(world, &a.entity)?;
    world.write_message(ApplyImpulse {
        entity,
        impulse: a.impulse,
        at: a.at,
        angular: a.angular,
    });
    Ok(json!({ "ok": true }))
}

#[derive(Deserialize)]
struct BlastArgs {
    center: DVec3,
    radius: f32,
    strength: f32,
    #[serde(default)]
    energy_j: f32,
    #[serde(default)]
    heat_c: f32,
}

fn blast(world: &mut World, args: Value) -> Result<Value, String> {
    let a: BlastArgs = parse(args)?;
    if a.radius <= 0.0 || a.strength <= 0.0 {
        return Err("radius and strength must be positive".into());
    }
    world.write_message(WorldImpulse {
        center: a.center,
        radius: a.radius,
        strength: a.strength,
        energy_j: a.energy_j,
        heat_c: a.heat_c,
        source: None,
    });
    Ok(json!({ "ok": true }))
}

fn stats(world: &mut World, _: Value) -> Result<Value, String> {
    serde_json::to_value(*world.resource::<PhysicsStats>()).map_err(|e| e.to_string())
}

/// Patch the debug layers (`all` first, then single fields) and return them.
fn debug(world: &mut World, args: Value) -> Result<Value, String> {
    let mut d = world.resource::<PhysicsDebug>().clone();
    if let Some(all) = args.get("all").and_then(Value::as_bool) {
        d.set_all(all);
    }
    let mut patched = serde_json::to_value(&d).map_err(|e| e.to_string())?;
    if let (Some(obj), Some(fields)) = (patched.as_object_mut(), args.as_object()) {
        for (k, v) in fields {
            if k == "all" {
                continue;
            }
            if !obj.contains_key(k) {
                return Err(format!(
                    "unknown field `{k}`; fields: {}",
                    obj.keys().cloned().collect::<Vec<_>>().join(", ")
                ));
            }
            obj.insert(k.clone(), v.clone());
        }
    }
    let d: PhysicsDebug = serde_json::from_value(patched).map_err(|e| e.to_string())?;
    *world.resource_mut::<PhysicsDebug>() = d.clone();
    serde_json::to_value(d).map_err(|e| e.to_string())
}

pub(crate) fn register(app: &mut App) {
    app.register_api(
        "physics.raycast",
        "First collider hit along a ray (world meters). Optional: max_distance (default 1000), \
         layers (bit mask), exclude (entity ids or names), include_sensors. Returns entity, \
         name, point, normal, distance and surface material, or {\"hit\": false}.",
        r#"{"origin": [0, 10, 0], "dir": [0, -1, 0], "max_distance": 50, "exclude": ["Player"]}"#,
        raycast,
    )
    .register_api(
        "physics.shapecast",
        "First collider hit when sweeping a shape along a direction. Shapes: Sphere, Cuboid, \
         Capsule, Cylinder, ConvexHull (see the Collider component). Same options as \
         physics.raycast, plus rotation [x, y, z, w].",
        r#"{"shape": {"Sphere": {"radius": 0.5}}, "origin": [0, 10, 0], "dir": [0, -1, 0], "max_distance": 50}"#,
        shapecast,
    )
    .register_api(
        "physics.overlap",
        "Every collider overlapping a shape placed at a position (optional rotation, layers, \
         exclude, include_sensors).",
        r#"{"shape": {"Cuboid": {"half_extents": [2, 1, 2]}}, "position": [0, 1, 0]}"#,
        overlap,
    )
    .register_api(
        "physics.closest_point",
        "Closest point on any collider within max_distance of a point; distance is negative \
         when the point is inside.",
        r#"{"point": [0, 1, 0], "max_distance": 5}"#,
        closest_point,
    )
    .register_api(
        "physics.impulse",
        "Kick a dynamic body: impulse in N·s (world axes), optional world point `at` (adds spin) \
         and angular impulse.",
        r#"{"entity": "Crate", "impulse": [0, 500, 0], "at": [0, 1, 0.5]}"#,
        impulse,
    )
    .register_api(
        "physics.blast",
        "Explosion at a point: pushes bodies (and wind, smoke, foliage, destruction through \
         WorldImpulse). radius in m, strength = peak air speed m/s; optional energy_j, heat_c.",
        r#"{"center": [0, 0.5, 0], "radius": 8, "strength": 120}"#,
        blast,
    )
    .register_api(
        "physics.stats",
        "What the last physics step did: bodies, awake, regions, touching pairs, step_ms, \
         substeps.",
        "{}",
        stats,
    )
    .register_api(
        "physics.debug",
        "Draw what physics sees (needs DebugPlugin): colliders, contacts, joints, regions, \
         centers_of_mass, velocities (bools; `all` sets every one), radius_m around the \
         focus, on_top. Returns the current settings; {} just reads them.",
        r#"{"colliders": true, "contacts": true, "joints": true}"#,
        debug,
    );
}
