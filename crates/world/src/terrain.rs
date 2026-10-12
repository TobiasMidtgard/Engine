//! The terrain-editing contract: destruction, digging, building and explosions ask for
//! terrain changes without knowing how terrain is stored (heightmap, voxels, planet).
//! The terrain module applies [`TerrainEdit`] and reports [`TerrainChanged`] so physics
//! colliders, foliage, navigation and water can update the affected area.

use engine_core::math::{DVec3, Quat, Vec3};
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

pub(crate) fn register(app: &mut App) {
    app.add_message::<TerrainEdit>()
        .add_message::<TerrainChanged>();
}
