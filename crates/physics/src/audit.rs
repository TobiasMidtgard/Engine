//! Collider audit: drawn meshes nothing collides with. `PHYSICS_COLLIDER_AUDIT=1` logs, every
//! [`EVERY_S`] seconds, the meshes of [`MIN_SIZE_M`] or more whose upper middle no collider
//! overlaps (a rock you walk through, a wall you can see through), by zone, biggest first;
//! `PHYSICS_COLLIDER_AUDIT=<file>.csv` also writes all of them found so far to that file.
//!
//! Each mesh is probed with a box a quarter of its size at the middle of its upper half: a
//! rock sitting on the terrain has its lower half in the ground's collider, its upper half
//! only in its own. What it can't tell: an arch or a ring (its middle is empty), a mesh whose
//! collider is a smaller stand-in on purpose (foliage, cloth), and meshes nothing should
//! collide with (sky, water, particles), so it lists candidates for a person to look over.
//! Playtests 2 (#15) and 3 (#13) asked for it: the waterfall's rocks, the scarp and the
//! valley's small boulders had no collider.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use engine_assets::Assets;
use engine_core::math::{DVec3, Quat, Vec3};
use engine_core::prelude::*;
use engine_render::{Mesh, MeshRef, MeshRenderer};
use engine_world::physics::{ColliderShape, PhysicsQuery, QueryFilter};

use crate::SharedSimulation;

/// Meshes smaller than this across (m) are left out (pebbles, props you may walk over).
pub const MIN_SIZE_M: f64 = 0.3;
/// Seconds between looks.
pub const EVERY_S: f64 = 10.0;
/// Meshes listed in the log per zone (the file has all).
const LOGGED: usize = 15;

/// What the audit found: zone → (item, where, size) keyed by item and place.
#[derive(Default)]
pub(crate) struct AuditState {
    last: Option<Instant>,
    found: BTreeMap<(String, String, [i64; 3]), (DVec3, f64)>,
}

/// One drawn mesh without a collider over it.
#[derive(Debug, Clone, PartialEq)]
pub struct Uncovered {
    pub entity: Entity,
    /// The zone: the topmost named ancestor.
    pub zone: String,
    /// The nearest name on it or an ancestor.
    pub item: String,
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
        let half = (hi - lo) * 0.5;
        let center = (lo + hi) * 0.5;
        let probe_at = center + DVec3::Y * half.y * 0.5;
        let probe = ColliderShape::Cuboid {
            half_extents: (half * 0.25).as_vec3().max(Vec3::splat(0.02)),
        };
        hits.clear();
        queries.overlap(&probe, probe_at, Quat::IDENTITY, &filter, &mut hits);
        if !hits.is_empty() {
            continue;
        }
        out.push(Uncovered {
            entity: e,
            zone: zone_of(world, e),
            item: crate::report::label(world, e),
            center,
            size,
        });
    }
    out.sort_by(|a, b| a.zone.cmp(&b.zone).then(b.size.total_cmp(&a.size)));
    out
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
    let mut by_zone: BTreeMap<&str, Vec<&Uncovered>> = BTreeMap::new();
    for u in &found {
        by_zone.entry(&u.zone).or_default().push(u);
    }
    for (zone, list) in &by_zone {
        let shown: Vec<String> = list
            .iter()
            .take(LOGGED)
            .map(|u| {
                format!(
                    "{} {:.1} m at ({:.0} {:.0} {:.0})",
                    u.item, u.size, u.center.x, u.center.y, u.center.z
                )
            })
            .collect();
        engine_core::log::info!(
            "collider audit: {zone}: {} drawn meshes of {MIN_SIZE_M} m or more with no collider \
             over them; biggest: {}",
            list.len(),
            shown.join(", ")
        );
    }
    engine_core::log::info!(
        "collider audit: {} meshes without a collider in {} zones ({:.1} ms)",
        found.len(),
        by_zone.len(),
        started.elapsed().as_secs_f64() * 1000.0
    );
    for u in found {
        let key = (
            u.zone,
            u.item,
            (u.center * 10.0).round().as_i64vec3().to_array(),
        );
        state.found.insert(key, (u.center, u.size));
    }
    let Some(path) = std::env::var_os("PHYSICS_COLLIDER_AUDIT") else {
        return;
    };
    let path = std::path::PathBuf::from(path);
    if path.extension().is_none_or(|x| x != "csv") {
        return;
    }
    let mut csv = String::from("zone,item,x,y,z,size_m\n");
    for ((zone, item, _), (c, size)) in &state.found {
        let clean = |s: &str| s.replace([',', '\n'], " ");
        csv.push_str(&format!(
            "{},{},{:.2},{:.2},{:.2},{:.2}\n",
            clean(zone),
            clean(item),
            c.x,
            c.y,
            c.z,
            size
        ));
    }
    if let Err(err) = std::fs::write(&path, csv) {
        engine_core::log::warn!("collider audit: can't write {}: {err}", path.display());
    }
}
