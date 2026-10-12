//! Scene queries (raycast, shapecast, overlap, closest point) across every region.

use std::sync::Arc;

use engine_core::math::{DVec3, Quat, Vec3};
use engine_core::prelude::*;
use engine_world::physics::{ColliderShape, Hit, PhysicsQuery, QueryFilter};
use parking_lot::RwLock;
use rapier3d::parry::query::ShapeCastOptions;
use rapier3d::prelude::{
    Collider as RapierCollider, ColliderHandle, Pose, QueryFilter as RFilter, Ray,
};

use crate::convert;
use crate::sim::{Region, Simulation};

/// The backend's [`PhysicsQuery`]: reads the simulation under a read lock, so it is safe from
/// any system or thread; it sees the state after the last physics step.
pub struct RapierQueries(pub Arc<RwLock<Simulation>>);

fn accepts(filter: &QueryFilter, c: &RapierCollider) -> bool {
    let e = convert::entity_of(c.user_data);
    (filter.layers == 0 || c.collision_groups().memberships.bits() & filter.layers != 0)
        && (filter.include_sensors || !c.is_sensor())
        && !filter.exclude.contains(&e)
}

fn hit(region: &Region, c: &RapierCollider, point: Vec3, normal: Vec3, distance: f32) -> Hit {
    Hit {
        entity: convert::entity_of(c.user_data),
        point: region.global(point),
        normal,
        distance,
        surface: convert::surface_of(c.user_data),
    }
}

/// Regions whose contents could intersect the segment `origin + dir·[0, max]` grown by `pad`.
fn regions_near<'a>(
    sim: &'a Simulation,
    origin: DVec3,
    dir: Vec3,
    max: f32,
    pad: f64,
) -> impl Iterator<Item = &'a Region> + 'a {
    let end = origin + (dir * max).as_dvec3();
    let (lo, hi) = (
        origin.min(end) - DVec3::splat(pad),
        origin.max(end) + DVec3::splat(pad),
    );
    sim.live_regions().filter_map(move |(_, r)| {
        let (rlo, rhi) = r.world_bounds()?;
        (rlo.cmple(hi).all() && lo.cmple(rhi).all()).then_some(r)
    })
}

impl PhysicsQuery for RapierQueries {
    fn raycast(
        &self,
        origin: DVec3,
        dir: Vec3,
        max_distance: f32,
        filter: &QueryFilter,
    ) -> Option<Hit> {
        let sim = self.0.read();
        let dir = dir.try_normalize()?;
        let pred = |_: ColliderHandle, c: &RapierCollider| accepts(filter, c);
        let mut best: Option<Hit> = None;
        for region in regions_near(&sim, origin, dir, max_distance, 0.0) {
            let ray = Ray::new(region.local(origin), dir);
            let max = best.map_or(max_distance, |b| b.distance);
            if let Some((h, i)) = region.world.cast_ray_and_get_normal(
                &ray,
                max,
                true,
                RFilter::new().predicate(&pred),
            ) {
                let c = &region.world.colliders[h];
                best = Some(hit(
                    region,
                    c,
                    ray.point_at(i.time_of_impact),
                    i.normal,
                    i.time_of_impact,
                ));
            }
        }
        best
    }

    fn shapecast(
        &self,
        shape: &ColliderShape,
        origin: DVec3,
        rotation: Quat,
        dir: Vec3,
        max_distance: f32,
        filter: &QueryFilter,
    ) -> Option<Hit> {
        let s = convert::shape(shape, Vec3::ONE)?;
        let dir = dir.try_normalize()?;
        let sim = self.0.read();
        let pad = s.compute_local_aabb().half_extents().length() as f64;
        let pred = |_: ColliderHandle, c: &RapierCollider| accepts(filter, c);
        let mut best: Option<Hit> = None;
        for region in regions_near(&sim, origin, dir, max_distance, pad) {
            let pose = Pose::from_parts(region.local(origin), rotation);
            let max = best.map_or(max_distance, |b| b.distance);
            if let Some((h, cast)) = region.world.cast_shape(
                &pose,
                dir,
                &*s,
                ShapeCastOptions {
                    max_time_of_impact: max,
                    target_distance: 0.0,
                    stop_at_penetration: true,
                    compute_impact_geometry_on_penetration: true,
                },
                RFilter::new().predicate(&pred),
            ) {
                let c = &region.world.colliders[h];
                best = Some(hit(
                    region,
                    c,
                    cast.witness1,
                    cast.normal1,
                    cast.time_of_impact,
                ));
            }
        }
        best
    }

    fn overlap(
        &self,
        shape: &ColliderShape,
        position: DVec3,
        rotation: Quat,
        filter: &QueryFilter,
        out: &mut Vec<Entity>,
    ) {
        let Some(s) = convert::shape(shape, Vec3::ONE) else {
            return;
        };
        let sim = self.0.read();
        let pad = s.compute_local_aabb().half_extents().length() as f64;
        let pred = |_: ColliderHandle, c: &RapierCollider| accepts(filter, c);
        let start = out.len();
        for region in regions_near(&sim, position, Vec3::X, 0.0, pad) {
            let pose = Pose::from_parts(region.local(position), rotation);
            for (_, c) in region
                .world
                .intersect_shape(pose, &*s, RFilter::new().predicate(&pred))
            {
                let e = convert::entity_of(c.user_data);
                // Static colliders are copied into several regions: report each entity once.
                if !out[start..].contains(&e) {
                    out.push(e);
                }
            }
        }
    }

    fn closest_point(&self, point: DVec3, max_distance: f32, filter: &QueryFilter) -> Option<Hit> {
        let sim = self.0.read();
        let pred = |_: ColliderHandle, c: &RapierCollider| accepts(filter, c);
        let mut best: Option<Hit> = None;
        for region in regions_near(&sim, point, Vec3::X, 0.0, max_distance as f64) {
            let local = region.local(point);
            let max = best.map_or(max_distance, |b| b.distance.abs());
            if let Some((h, proj)) =
                region
                    .world
                    .project_point(local, max, false, RFilter::new().predicate(&pred))
            {
                let c = &region.world.colliders[h];
                let to_point = local - proj.point;
                let dist = to_point.length();
                let normal = if proj.is_inside {
                    -to_point.try_normalize().unwrap_or(Vec3::Y)
                } else {
                    to_point.try_normalize().unwrap_or(Vec3::Y)
                };
                let signed = if proj.is_inside { -dist } else { dist };
                if best.is_none_or(|b| signed < b.distance) {
                    best = Some(hit(region, c, proj.point, normal, signed));
                }
            }
        }
        best
    }
}
