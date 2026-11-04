//! Collider audit: drawn meshes nothing collides with. `PHYSICS_COLLIDER_AUDIT=1` logs, every
//! [`EVERY_S`] seconds, the meshes of [`MIN_SIZE_M`] or more whose upper middle no collider
//! overlaps (a rock you walk through, a wall you can see through), by zone, biggest first;
//! `PHYSICS_COLLIDER_AUDIT=<file>.csv` also writes all of them found so far to that file.
//!
//! Each mesh is probed at up to [`SAMPLES`] points of its drawn surface in its upper half
//! (the lower half of a rock is in the ground's collider): it counts as covered if a collider
//! comes within [`TOUCH_M`] of half of them or more. So a hall colliding through its walls, a
//! mesh merged from many masts, or an arch, are judged by where they are drawn.
//!
//! Left out: meshes with a body, a collider or a character on them or an ancestor (a vehicle,
//! cloth, a tree whose collider is a smaller stand-in on purpose); the kinds and names
//! [`SKIPPED`] lists (foliage parts, LOD ghosts, NPC stand-ins, debris, far stand-ins) plus
//! those `PHYSICS_COLLIDER_AUDIT_SKIP` lists (comma separated); and meshes farther than
//! [`NEAR_M`] from every focus point (colliders streamed in near the action). What it can't
//! tell is a mesh nothing should collide with (sky, water), so it lists candidates for a
//! person to look over, grouped by mesh: the same rock placed 40 times is one row.
//!
//! Playtests 2 (#15) and 3 (#13) asked for it: the waterfall's rocks, the scarp and the
//! valley's small boulders had no collider.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use engine_assets::Assets;
use engine_core::math::{DVec3, Quat, Vec3};
use engine_core::prelude::*;
use engine_render::{Mesh, MeshRef, MeshRenderer};
use engine_world::physics::{
    CharacterController, Collider, ColliderShape, FitCollider, PhysicsQuery, QueryFilter, RigidBody,
};

use crate::SharedSimulation;

/// Meshes smaller than this across (m) are left out (pebbles, props you may walk over).
pub const MIN_SIZE_M: f64 = 0.3;
/// Points of a mesh's surface looked at, at most.
const SAMPLES: usize = 24;
/// A point of the surface is solid if a collider is this close (m): a hull round a rock's far
/// mesh, a box round a wall, sit this near what is drawn.
const TOUCH_M: f32 = 0.2;
/// Meshes farther than this (m) from every focus point (`engine_world::FocusPoints`, else every
/// character) are not looked at: showcase scenery gets its colliders only near a focus point
/// (`NearSolid`, within 70 m), as streaming worlds do. With neither, every mesh is.
pub const NEAR_M: f64 = 30.0;
/// Seconds between looks.
pub const EVERY_S: f64 = 10.0;
/// Meshes listed in the log per zone (the file has all).
const LOGGED: usize = 15;
/// Component types (the last part of their names), or entity names, whose meshes are left
/// out: drawn parts with their own way of colliding, or none on purpose (foliage parts and
/// shadow stand-ins, LOD fade ghosts, NPC stand-ins and flickering lights, falling debris,
/// shadow-only meshes, far stand-ins).
pub const SKIPPED: &[&str] = &[
    "PlantPart",
    "ShadowStandIn",
    "FadeGhost",
    "StandInPart",
    "Flicker",
    "Debris",
    "ShadowOnly",
    // Named: the showcase's far stand-ins for whole districts (HLOD).
    "hlod proxy",
];

/// Every mesh found so far, by (zone, item, mesh, place to a decimetre): where it is, its size.
type Found = BTreeMap<(String, String, String, [i64; 3]), (DVec3, f64)>;

#[derive(Default)]
pub(crate) struct AuditState {
    last: Option<Instant>,
    found: Found,
}

/// One drawn mesh without a collider over it.
#[derive(Debug, Clone, PartialEq)]
pub struct Uncovered {
    pub entity: Entity,
    /// The zone: the topmost named ancestor.
    pub zone: String,
    /// The nearest name on it or an ancestor.
    pub item: String,
    /// The mesh: its asset path, or its handle.
    pub mesh: String,
    pub center: DVec3,
    /// Its largest extent, m.
    pub size: f64,
}

/// The drawn meshes of [`MIN_SIZE_M`] or more that no collider covers (module docs).
pub fn uncovered_meshes(world: &World) -> Vec<Uncovered> {
    let (Some(meshes), Some(sim)) = (
        world.get_resource::<Assets<Mesh>>(),
        world.get_resource::<SharedSimulation>(),
    ) else {
        return Vec::new();
    };
    let queries = crate::queries::RapierQueries(sim.0.clone());
    let Some(mut q) = world.try_query::<(Entity, &MeshRenderer, &GlobalTransform)>() else {
        return Vec::new();
    };
    let filter = QueryFilter::default();
    let mut skipped: Vec<String> = SKIPPED.iter().map(|s| s.to_string()).collect();
    if let Some(more) = std::env::var_os("PHYSICS_COLLIDER_AUDIT_SKIP") {
        skipped.extend(
            more.to_string_lossy()
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty()),
        );
    }
    // Where colliders are streamed in around (the player, the camera); with none, characters.
    let focus: Vec<DVec3> = match world.get_resource::<engine_world::FocusPoints>() {
        Some(f) if !f.points.is_empty() => f.points.clone(),
        _ => world
            .try_query::<(&CharacterController, &GlobalTransform)>()
            .map(|mut q| q.iter(world).map(|(_, g)| g.translation()).collect())
            .unwrap_or_default(),
    };
    let mut out = Vec::new();
    let mut hits = Vec::new();
    for (e, renderer, global) in q.iter(world) {
        if !renderer.visible {
            continue;
        }
        let handle = match &renderer.mesh {
            MeshRef::Handle(h) => Some(*h),
            MeshRef::Asset(key) => meshes.loaded_by_key(&engine_assets::normalize_path(key)),
            MeshRef::Shape(_) => None,
        };
        let Some(mesh) = handle.and_then(|h| meshes.get(h)) else {
            continue;
        };
        let aabb = mesh.aabb();
        // World bounds of the mesh's box.
        let (mut lo, mut hi) = (DVec3::splat(f64::INFINITY), DVec3::splat(f64::NEG_INFINITY));
        for i in 0..8 {
            let corner = Vec3::new(
                if i & 1 == 0 { aabb.min.x } else { aabb.max.x },
                if i & 2 == 0 { aabb.min.y } else { aabb.max.y },
                if i & 4 == 0 { aabb.min.z } else { aabb.max.z },
            );
            let p = global.0.transform_point3(corner.as_dvec3());
            lo = lo.min(p);
            hi = hi.max(p);
        }
        let size = (hi - lo).max_element();
        if !size.is_finite() || size < MIN_SIZE_M {
            continue;
        }
        let near = |p: &DVec3| p.clamp(lo, hi).distance(*p) <= NEAR_M;
        if !focus.is_empty() && !focus.iter().any(near) {
            continue;
        }
        let center = (lo + hi) * 0.5;
        // Points of the drawn surface in its upper half, spread over the mesh.
        let upper: Vec<DVec3> = mesh
            .vertices
            .iter()
            .map(|v| global.0.transform_point3(Vec3::from(v.position).as_dvec3()))
            .filter(|p| p.y >= center.y)
            .collect();
        let stride = upper.len().div_ceil(SAMPLES).max(1);
        let ball = ColliderShape::Sphere { radius: TOUCH_M };
        let (mut tried, mut touched) = (0, 0);
        for p in upper.iter().step_by(stride) {
            hits.clear();
            queries.overlap(&ball, *p, Quat::IDENTITY, &filter, &mut hits);
            tried += 1;
            touched += usize::from(!hits.is_empty());
        }
        if touched * 2 >= tried || physical(world, e) || of_skipped_kind(world, e, &skipped) {
            continue;
        }
        out.push(Uncovered {
            entity: e,
            zone: zone_of(world, e),
            item: kind(crate::report::label(world, e)),
            mesh: match &renderer.mesh {
                MeshRef::Asset(key) => key.to_string(),
                _ => handle.map_or_else(String::new, |h| format!("{h:?}")),
            },
            center,
            size,
        });
    }
    out.sort_by(|a, b| a.zone.cmp(&b.zone).then(b.size.total_cmp(&a.size)));
    out
}

/// A label without the entity id an unnamed one starts with, so like meshes group together.
fn kind(label: String) -> String {
    let first = label.split(' ').next().unwrap_or("");
    let is_id = first.split_once('v').is_some_and(|(i, g)| {
        !i.is_empty() && !g.is_empty() && (i.to_owned() + g).bytes().all(|b| b.is_ascii_digit())
    });
    if !is_id {
        return label;
    }
    let rest = label[first.len()..].trim();
    if rest.is_empty() {
        "(unnamed)".into()
    } else {
        rest.into()
    }
}

/// Does `e` or an ancestor have a body, a collider or a character on it?
fn physical(world: &World, e: Entity) -> bool {
    let mut cur = e;
    loop {
        let r = world.entity(cur);
        if r.contains::<RigidBody>()
            || r.contains::<Collider>()
            || r.contains::<FitCollider>()
            || r.contains::<CharacterController>()
        {
            return true;
        }
        match world.get::<ChildOf>(cur) {
            Some(p) => cur = p.parent(),
            None => return false,
        }
    }
}

/// Is `e` named one of `skipped`, or has it a component whose type name ends in one?
fn of_skipped_kind(world: &World, e: Entity, skipped: &[String]) -> bool {
    if let Some(name) = world.get::<Name>(e)
        && skipped.iter().any(|s| s == name.as_str())
    {
        return true;
    }
    let Ok(components) = world.inspect_entity(e) else {
        return false;
    };
    components.into_iter().any(|(_, info)| {
        let name = info.name().to_string();
        let path = name.split('<').next().unwrap_or(&name);
        let ty = path.rsplit("::").next().unwrap_or(path);
        skipped.iter().any(|s| s == ty)
    })
}

/// The topmost named ancestor of `e` (its zone in the showcase), or "(no zone)".
fn zone_of(world: &World, e: Entity) -> String {
    let mut cur = e;
    let mut zone = None;
    loop {
        if let Some(name) = world.get::<Name>(cur) {
            zone = Some(name.as_str().to_string());
        }
        match world.get::<ChildOf>(cur) {
            Some(p) => cur = p.parent(),
            None => break,
        }
    }
    zone.unwrap_or_else(|| "(no zone)".into())
}

/// Every [`EVERY_S`] seconds: log what [`uncovered_meshes`] finds, and write the file
/// (module docs). Added only when `PHYSICS_COLLIDER_AUDIT` is set.
pub(crate) fn audit_colliders(world: &mut World, mut state: Local<AuditState>) {
    if state
        .last
        .is_some_and(|t| t.elapsed() < Duration::from_secs_f64(EVERY_S))
    {
        return;
    }
    state.last = Some(Instant::now());
    let started = Instant::now();
    let found = uncovered_meshes(world);
    let count = found.len();
    for u in found {
        let key = (
            u.zone,
            u.item,
            u.mesh,
            (u.center * 10.0).round().as_i64vec3().to_array(),
        );
        state.found.insert(key, (u.center, u.size));
    }
    let groups = grouped(&state.found);
    let mut by_zone: BTreeMap<&str, Vec<&Group>> = BTreeMap::new();
    for g in &groups {
        by_zone.entry(&g.zone).or_default().push(g);
    }
    for (zone, list) in &by_zone {
        let shown: Vec<String> = list
            .iter()
            .take(LOGGED)
            .map(|g| {
                format!(
                    "{} x{} {:.1} m at ({:.0} {:.0} {:.0})",
                    g.item, g.count, g.max, g.at.x, g.at.y, g.at.z
                )
            })
            .collect();
        engine_core::log::info!(
            "collider audit: {zone}: {} kinds of drawn mesh of {MIN_SIZE_M} m or more with no \
             collider over them ({} placed); biggest: {}",
            list.len(),
            list.iter().map(|g| g.count).sum::<usize>(),
            shown.join(", ")
        );
    }
    engine_core::log::info!(
        "collider audit: {count} meshes without a collider in view now, {} kinds in {} zones \
         so far ({:.1} ms)",
        groups.len(),
        by_zone.len(),
        started.elapsed().as_secs_f64() * 1000.0
    );
    let Some(path) = std::env::var_os("PHYSICS_COLLIDER_AUDIT") else {
        return;
    };
    let path = std::path::PathBuf::from(path);
    if path.extension().is_none_or(|x| x != "csv") {
        return;
    }
    let clean = |s: &str| s.replace([',', '\n'], " ");
    let mut csv = String::from("zone,item,mesh,count,min_size_m,max_size_m,x,y,z\n");
    for g in &groups {
        csv.push_str(&format!(
            "{},{},{},{},{:.2},{:.2},{:.2},{:.2},{:.2}\n",
            clean(&g.zone),
            clean(&g.item),
            clean(&g.mesh),
            g.count,
            g.min,
            g.max,
            g.at.x,
            g.at.y,
            g.at.z
        ));
    }
    if let Err(err) = std::fs::write(&path, csv) {
        engine_core::log::warn!("collider audit: can't write {}: {err}", path.display());
    }
}

/// The meshes found of one kind in one zone: the same item drawn with the same mesh.
struct Group {
    zone: String,
    item: String,
    mesh: String,
    count: usize,
    min: f64,
    max: f64,
    /// Where the biggest one is.
    at: DVec3,
}

/// `found` grouped by zone, item and mesh: by zone, then biggest first.
fn grouped(found: &Found) -> Vec<Group> {
    let mut groups: Vec<Group> = Vec::new();
    for ((zone, item, mesh, _), &(at, size)) in found {
        match groups.last_mut() {
            Some(g) if g.zone == *zone && g.item == *item && g.mesh == *mesh => {
                g.count += 1;
                g.min = g.min.min(size);
                if size > g.max {
                    g.max = size;
                    g.at = at;
                }
            }
            _ => groups.push(Group {
                zone: zone.clone(),
                item: item.clone(),
                mesh: mesh.clone(),
                count: 1,
                min: size,
                max: size,
                at,
            }),
        }
    }
    groups.sort_by(|a, b| a.zone.cmp(&b.zone).then(b.max.total_cmp(&a.max)));
    groups
}
