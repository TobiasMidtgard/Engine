//! Virtual geometry: dense meshes drawn as clusters with per-cluster level of detail, culled and
//! selected on the GPU (Nanite-style, Karis 2021).
//!
//! ## What happens to a dense mesh
//!
//! 1. **Build** (CPU, background thread, once per mesh version): the mesh is split into
//!    clusters of up to [`CLUSTER_TRIANGLES`] triangles. Neighboring clusters are grouped
//!    ([`GROUP_SIZE`] at a time, preferring the clusters that share the most edges), each
//!    group is simplified to half its triangles with its outer border locked, and the result
//!    is split into new clusters. Repeating this level by level builds a DAG from the full
//!    mesh down to a single coarse cluster ([`build`], [`ClusterDag`]).
//! 2. **Errors**: every cluster stores its own simplification error (meters, 0 for the
//!    original triangles) over a bounding sphere, and the error and sphere of the group
//!    simplified *from* it (its parent). Errors and spheres only grow toward the root, so for
//!    any camera there is exactly one cut through the DAG: draw a cluster when its own error
//!    projects to at most the pixel threshold and its parent's error doesn't. Clusters on both
//!    sides of a group border use the same locked vertices, so a cut that mixes levels never
//!    cracks.
//! 3. **Draw** (GPU, every frame, [`ClusterRenderer`]): a compute pass culls instances against
//!    each view (camera, shadow cascades, sky occlusion), picks clusters by projected error,
//!    culls them against the frustum and, for the camera, by their normal cone (backfacing),
//!    and appends the survivors to a visible-cluster list with indirect draw arguments. The
//!    clusters are then drawn with hardware rasterization into the depth prepass, the scene
//!    pass (the same `StandardMaterial` shading as every other mesh) and the shadow maps.
//!
//! Only 32-bit atomics, storage buffers and indirect draws/dispatches are used, so it runs on
//! every wgpu backend (including lavapipe and WebGPU).
//!
//! ## Which meshes take this path
//!
//! Automatic: a [`MeshRenderer`](crate::MeshRenderer) whose mesh has at least
//! `RenderSettings::virtual_geometry_min_triangles` triangles and whose material is opaque is
//! drawn as clusters once its build has finished (until then, and whenever the mesh changes
//! version, it is drawn by the regular instanced path, so nothing pops in or disappears).
//! Smaller meshes, alpha-masked and transparent materials stay on the instanced path.
//! `RenderSettings::virtual_geometry = false` turns the cluster path off.

mod build;
mod simplify;

pub use build::build;

use engine_core::math::Vec3;

use crate::mesh::Vertex;

/// Most triangles in one cluster.
pub const CLUSTER_TRIANGLES: usize = 128;
/// Most vertices in one cluster (triangle corners are stored as 8-bit cluster-local indices).
pub const CLUSTER_MAX_VERTICES: usize = 256;
/// Clusters simplified together in a group.
pub const GROUP_SIZE: usize = 8;
/// Parent error of the root clusters (they have no coarser version): larger than any
/// projected threshold, and finite so GPUs don't have to handle infinities.
pub const ROOT_ERROR: f32 = 1.0e30;

/// A bounding sphere, in mesh space (meters).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Sphere {
    pub center: Vec3,
    pub radius: f32,
}

impl Sphere {
    /// A sphere around points: centered on their bounding box, through the farthest point.
    pub fn around_points(points: impl Iterator<Item = Vec3> + Clone) -> Self {
        let (mut lo, mut hi) = (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY));
        for p in points.clone() {
            lo = lo.min(p);
            hi = hi.max(p);
        }
        if lo.x > hi.x {
            return Self::default();
        }
        let center = (lo + hi) * 0.5;
        let radius = points.map(|p| p.distance(center)).fold(0.0, f32::max);
        Self { center, radius }
    }

    /// A sphere around spheres.
    pub fn enclosing(spheres: impl Iterator<Item = Sphere> + Clone) -> Self {
        let (mut lo, mut hi) = (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY));
        for s in spheres.clone() {
            lo = lo.min(s.center - s.radius);
            hi = hi.max(s.center + s.radius);
        }
        if lo.x > hi.x {
            return Self::default();
        }
        let center = (lo + hi) * 0.5;
        let radius = spheres
            .map(|s| s.center.distance(center) + s.radius)
            .fold(0.0, f32::max);
        Self { center, radius }
    }

    /// Whether `other` lies inside this sphere (with a small tolerance for rounding).
    pub fn contains(&self, other: &Sphere) -> bool {
        other.center.distance(self.center) + other.radius <= self.radius * (1.0 + 1e-5) + 1e-6
    }

    pub fn to_array(self) -> [f32; 4] {
        [self.center.x, self.center.y, self.center.z, self.radius]
    }
}

/// One cluster: up to [`CLUSTER_TRIANGLES`] triangles of one level of detail.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cluster {
    /// Where its data starts in [`ClusterDag::indices`]: `vertex_count` indices into
    /// [`ClusterDag::vertices`], then `triangle_count` triangles packed as three 8-bit
    /// cluster-local corner indices (`a | b << 8 | c << 16`, counter-clockwise).
    pub index_offset: u32,
    pub vertex_count: u32,
    pub triangle_count: u32,
    /// Simplification level it was made at (0 = the original triangles).
    pub level: u32,
    /// Bounds of its triangles (culling).
    pub bounds: Sphere,
    /// Normal cone: every triangle's normal is within `acos(cone_cos)` of `cone_axis`.
    /// `cone_cos <= 0` means the cluster faces too many ways to be culled as backfacing.
    pub cone_axis: Vec3,
    pub cone_cos: f32,
    /// How far (meters) its surface may be from the original mesh, measured over
    /// `lod_bounds` (shared by every cluster made from the same group).
    pub error: f32,
    pub lod_bounds: Sphere,
    /// Error and sphere of the coarser clusters simplified from this one's group, or
    /// [`ROOT_ERROR`] at the root.
    pub parent_error: f32,
    pub parent_bounds: Sphere,
}

/// The clusters made at one simplification level (a contiguous range of
/// [`ClusterDag::clusters`]), with error bounds the GPU uses to skip whole levels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClusterLevel {
    pub first: u32,
    pub count: u32,
    /// Smallest `error` of its clusters.
    pub min_error: f32,
    /// Largest `parent_error` of its clusters.
    pub max_parent_error: f32,
}

/// A mesh as a hierarchy of clusters at every level of detail. Made by [`build`].
#[derive(Debug, Clone, Default)]
pub struct ClusterDag {
    /// The mesh's vertices (identical vertices merged). Every level uses these: simplification
    /// only removes vertices, it never moves them.
    pub vertices: Vec<Vertex>,
    /// Per cluster: its vertex indices, then its packed triangles (see [`Cluster`]).
    pub indices: Vec<u32>,
    /// Ordered by level, finest first.
    pub clusters: Vec<Cluster>,
    pub levels: Vec<ClusterLevel>,
    /// Encloses every cluster and every error sphere.
    pub bounds: Sphere,
    /// Triangles of the original mesh (after dropping degenerate ones).
    pub source_triangles: u32,
}

impl ClusterDag {
    /// The triangles of cluster `c` as indices into [`vertices`](Self::vertices).
    pub fn triangles(&self, c: usize) -> impl Iterator<Item = [u32; 3]> + '_ {
        let cl = &self.clusters[c];
        let refs =
            &self.indices[cl.index_offset as usize..(cl.index_offset + cl.vertex_count) as usize];
        let start = (cl.index_offset + cl.vertex_count) as usize;
        self.indices[start..start + cl.triangle_count as usize]
            .iter()
            .map(move |&packed| [0, 8, 16].map(|shift| refs[((packed >> shift) & 0xff) as usize]))
    }

    /// Clusters with no coarser version.
    pub fn roots(&self) -> impl Iterator<Item = usize> + '_ {
        self.clusters
            .iter()
            .enumerate()
            .filter(|(_, c)| c.parent_error >= ROOT_ERROR)
            .map(|(i, _)| i)
    }

    /// The cut through the DAG for a view: clusters whose own error projects to at most
    /// `threshold` while their parent's projects to more. `eye` is the viewpoint in mesh
    /// space and `scale` turns meters at one meter's distance into pixels; with `eye = None`
    /// errors are compared directly (`error * scale`, an orthographic view). The GPU culling
    /// shader makes the same choice per cluster.
    pub fn select(&self, eye: Option<Vec3>, scale: f32, threshold: f32) -> Vec<usize> {
        (0..self.clusters.len())
            .filter(|&i| {
                let c = &self.clusters[i];
                projected_error(c.error, c.lod_bounds, eye, scale) <= threshold
                    && projected_error(c.parent_error, c.parent_bounds, eye, scale) > threshold
            })
            .collect()
    }
}

/// Smallest distance at which errors are projected (closer, a cluster counts as this close).
pub const MIN_PROJECTION_DISTANCE: f32 = 1e-3;

/// Error (meters) seen from `eye` as pixels: `error * scale / distance` to the nearest point of
/// `sphere` (`error * scale` without an eye). Monotonic: a larger error over a sphere that
/// contains the other never projects smaller.
pub fn projected_error(error: f32, sphere: Sphere, eye: Option<Vec3>, scale: f32) -> f32 {
    match eye {
        Some(eye) => {
            let d = (sphere.center.distance(eye) - sphere.radius).max(MIN_PROJECTION_DISTANCE);
            error * scale / d
        }
        None => error * scale,
    }
}
