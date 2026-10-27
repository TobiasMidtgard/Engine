//! The [`Ground`](engine_world::Ground) query for terrains: height, normal and surface
//! material at any point, from the same shape and edits the renderer draws.

use std::sync::Arc;

use engine_core::math::{DQuat, DVec3, Vec3};
use engine_world::{
    ForestQuery, ForestSample, GroundQuery, GroundSample, SurfaceId, SurfaceMaterials,
};

use crate::chunk::ChunkSource;
use crate::palette::{SurfaceIds, TerrainPalette};

/// Another ground query that names the surface material on a terrain, while the terrain
/// still answers height and normal (see [`PlanetTerrain::with_ground_surface`]).
///
/// [`PlanetTerrain::with_ground_surface`]: crate::PlanetTerrain::with_ground_surface
#[derive(Clone)]
pub struct GroundSurface(pub Arc<dyn GroundQuery>);

impl std::fmt::Debug for GroundSurface {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("GroundSurface(..)")
    }
}

/// The same query object (setting a new one counts as a change and rebuilds the terrain).
impl PartialEq for GroundSurface {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

/// One terrain as the ground query sees it.
#[derive(Clone)]
pub struct GroundTerrain {
    pub source: ChunkSource,
    pub palette: TerrainPalette,
    pub position: DVec3,
    pub rotation: DQuat,
    /// Names the surface material instead of the palette (see [`GroundSurface`]).
    pub surface_override: Option<GroundSurface>,
}

impl GroundTerrain {
    /// Full-detail surface under (or over) a world point: the world surface point along the
    /// local vertical through `p`, the world normal and the surface material.
    pub fn surface(&self, p: DVec3, ids: &SurfaceIds) -> Option<(DVec3, Vec3, SurfaceId, f64)> {
        let inv = self.rotation.inverse();
        let local = inv * (p - self.position);
        let (dir, up, base) = self.source.map.locate(local)?;
        // Solid ground, also under the sea (the sea itself is the water module's `Water`).
        let s = self.source.sample(base, up, dir, 0.0);
        let point_local = base + up * s.height;
        let n = self.local_normal(base, up, point_local, 0.5, 0.0);
        let slope = 1.0 - n.dot(up);
        let under_sea = s.height < 0.0 && self.source.shape.ocean;
        let mut surface = self.palette.classify(&s, slope, under_sea, ids);
        let world_point = self.position + self.rotation * point_local;
        let world_normal = (self.rotation * n).as_vec3();
        // The override names the ground, except for the sea and for layers edits painted
        // (a dug pit or a mud splash is newer than the landscape's materials).
        if let Some(o) = &self.surface_override
            && !under_sea
            && !(self.source.edits.touches(point_local, 0.5)
                && self.source.edit_paint(base, up, dir).1 > 0.5)
            && let Some(g) = o.0.sample(world_point + self.rotation * up * 0.5)
        {
            surface = g.surface;
        }
        Some((world_point, world_normal, surface, s.height))
    }

    /// World point of the surface along the local vertical through `p`: what [`Self::surface`]
    /// finds, without the normal's two extra samples or the surface material.
    pub fn surface_point(&self, p: DVec3) -> Option<DVec3> {
        let local = self.rotation.inverse() * (p - self.position);
        let (dir, up, base) = self.source.map.locate(local)?;
        let s = self.source.sample(base, up, dir, 0.0);
        Some(self.position + self.rotation * (base + up * s.height))
    }

    /// Terrain-local ground normal at `point_local` (the surface over `base`), from two
    /// samples `e` meters away with detail down to `min_wavelength`.
    pub(crate) fn local_normal(
        &self,
        base: DVec3,
        up: DVec3,
        point_local: DVec3,
        e: f64,
        min_wavelength: f64,
    ) -> DVec3 {
        let t1 = up.any_orthonormal_vector();
        let t2 = up.cross(t1);
        let probe = |offset: DVec3| -> DVec3 {
            let q = base + offset;
            match self.source.map.locate(q) {
                Some((d, u, b)) => b + u * self.source.sample(b, u, d, min_wavelength).height,
                None => q,
            }
        };
        let pa = probe(t1 * e);
        let pb = probe(t2 * e);
        let n = (pa - point_local).cross(pb - point_local).normalize_or(up);
        if n.dot(up) < 0.0 { -n } else { n }
    }
}

/// The ground provider installed in [`engine_world::Ground`] while terrains exist.
pub struct TerrainGround {
    pub terrains: Vec<GroundTerrain>,
    pub ids: SurfaceIds,
}

impl TerrainGround {
    pub fn new(terrains: Vec<GroundTerrain>, materials: &SurfaceMaterials) -> Self {
        Self {
            terrains,
            ids: SurfaceIds::new(materials),
        }
    }

    /// The terrain whose surface is nearest to `p`.
    fn nearest(&self, p: DVec3) -> Option<&GroundTerrain> {
        self.terrains.iter().min_by(|a, b| {
            let da = (p - a.position).length() - a.source.map.radius;
            let db = (p - b.position).length() - b.source.map.radius;
            let da = if a.source.map.is_sphere() {
                da
            } else {
                (p - a.position).y
            };
            let db = if b.source.map.is_sphere() {
                db
            } else {
                (p - b.position).y
            };
            da.abs().total_cmp(&db.abs())
        })
    }
}

impl GroundQuery for TerrainGround {
    fn sample(&self, p: DVec3) -> Option<GroundSample> {
        let t = self.nearest(p)?;
        let (point, normal, surface, _) = t.surface(p, &self.ids)?;
        // The contract reports a world height for "the ground below X/Z". Near a planet's top
        // the local vertical is world +Y; elsewhere this is the height of the surface point on
        // the radial line through `p`.
        Some(GroundSample {
            height: point.y,
            normal,
            surface,
        })
    }

    /// Sight lines, footing and soft bodies ask only the height, often thousands of times a
    /// frame: one terrain sample instead of three plus the surface override's own query.
    fn height(&self, p: DVec3) -> Option<f64> {
        Some(self.nearest(p)?.surface_point(p)?.y)
    }
}

/// The same terrains answer [`engine_world::ForestCover`]: woods where their ground shows forest.
impl ForestQuery for TerrainGround {
    fn sample(&self, p: DVec3) -> Option<ForestSample> {
        self.nearest(p)?.forest(p)
    }
}

/// Convenience: an `Arc` ground provider.
pub fn provider(terrains: Vec<GroundTerrain>, materials: &SurfaceMaterials) -> Arc<TerrainGround> {
    Arc::new(TerrainGround::new(terrains, materials))
}
