//! The terrain-editing contract: destruction, digging, building and explosions ask for
//! terrain changes without knowing how terrain is stored (heightmap, voxels, planet).
//! The terrain module applies [`TerrainEdit`] and reports [`TerrainChanged`] so physics
//! colliders, foliage, navigation and water can update the affected area.
//!
//! It also answers where the terrain's creeks run ([`Creeks`]), so water can draw them, foliage
//! can grow alder, willow and sedge along them and audio can hear them, without knowing how the
//! terrain carves its stream network.

use std::sync::Arc;

use engine_core::math::{DVec2, DVec3, Quat, Vec2, Vec3};
use engine_core::prelude::*;

use crate::physical::SurfaceId;

/// Region of an edit, centered on [`TerrainEdit::center`].
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum EditShape {
    Sphere {
        radius: f32,
    },
    Box {
        half_extents: Vec3,
        rotation: Quat,
    },
    /// Tunnel or trench from `center` to `center + to`.
    Capsule {
        to: Vec3,
        radius: f32,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum EditOp {
    /// Dig, blast, carve.
    Remove,
    /// Build up, fill (with `surface`).
    Add,
    /// Soften edges (0..1).
    Smooth { strength: f32 },
    /// Change the surface material only (scorch, paint a path).
    Paint,
}

/// Change the terrain.
#[derive(Message, Debug, Clone, Copy, PartialEq)]
pub struct TerrainEdit {
    pub center: DVec3,
    pub shape: EditShape,
    pub op: EditOp,
    /// Material for `Add`/`Paint`; `None` = keep or use the default.
    pub surface: Option<SurfaceId>,
    pub source: Option<Entity>,
}

/// Terrain inside this sphere changed (after edits are applied).
#[derive(Message, Debug, Clone, Copy, PartialEq)]
pub struct TerrainChanged {
    pub center: DVec3,
    pub radius: f32,
}

/// The creek nearest a point.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CreekSample {
    /// Distance from the point to the creek's center line on the ground (X, Z), meters.
    pub distance_m: f32,
    /// Nearest point of the center line, at the water surface.
    pub center: DVec3,
    /// Downstream direction there, a unit vector on the ground (X, Z).
    pub downstream: Vec2,
    /// Width of the water surface, meters.
    pub width_m: f32,
    /// Depth of water over the deepest part of the bed, meters.
    pub depth_m: f32,
    /// World height of the bed's deepest point there, meters.
    pub bed_m: f64,
    /// Discharge, m³/s (rain and snowmelt may raise it; this is the terrain's base flow).
    pub flow_m3_s: f32,
    /// Strahler stream order: 1 for a headwater creek, one more where two of the same order
    /// meet.
    pub order: u8,
}

impl CreekSample {
    /// Whether the point is over the water (inside the creek's width).
    pub fn in_water(&self) -> bool {
        self.distance_m <= 0.5 * self.width_m
    }

    /// World height of the water surface there, meters.
    pub fn surface_m(&self) -> f64 {
        self.center.y
    }
}

/// One point of a creek's center line, at the water surface.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CreekPoint {
    pub at: DVec3,
    pub width_m: f32,
    pub depth_m: f32,
    pub flow_m3_s: f32,
}

/// A stretch of creek between two junctions (or a source and a junction), as a polyline from
/// upstream to downstream.
#[derive(Debug, Clone, PartialEq)]
pub struct CreekReach {
    /// Stable while the terrain is unchanged, so readers can keep what they built for it.
    pub id: u64,
    pub order: u8,
    pub points: Vec<CreekPoint>,
}

/// Implemented by the terrain module (its stream network).
pub trait CreekQuery: Send + Sync + 'static {
    /// The creek whose center line passes nearest `p` (X, Z), if one is within `within_m`.
    fn nearest(&self, p: DVec3, within_m: f32) -> Option<CreekSample>;

    /// Every reach with water inside the box `lo`..`hi` (world X, Z), added to `out`, whole
    /// (a reach that crosses the box's edge is given entire).
    fn reaches(&self, lo: DVec2, hi: DVec2, out: &mut Vec<CreekReach>);
}

/// The active creek provider. Defaults to no creeks anywhere.
#[derive(Resource, Clone)]
pub struct Creeks(pub Arc<dyn CreekQuery>);

struct NoCreeks;
impl CreekQuery for NoCreeks {
    fn nearest(&self, _p: DVec3, _within_m: f32) -> Option<CreekSample> {
        None
    }

    fn reaches(&self, _lo: DVec2, _hi: DVec2, _out: &mut Vec<CreekReach>) {}
}

impl Default for Creeks {
    fn default() -> Self {
        Self(Arc::new(NoCreeks))
    }
}

impl Creeks {
    pub fn nearest(&self, p: DVec3, within_m: f32) -> Option<CreekSample> {
        self.0.nearest(p, within_m)
    }

    pub fn reaches(&self, lo: DVec2, hi: DVec2, out: &mut Vec<CreekReach>) {
        self.0.reaches(lo, hi, out);
    }
}

pub(crate) fn register(app: &mut App) {
    app.add_message::<TerrainEdit>()
        .add_message::<TerrainChanged>()
        .init_resource::<Creeks>();
}
