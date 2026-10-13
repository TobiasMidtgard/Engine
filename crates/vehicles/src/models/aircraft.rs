//! The sport plane and the spaceplane: fuselages, wings, control surfaces, canopies, engines,
//! landing gear and their materials.
//!
//! Each model builds its meshes, textures and materials once per world (registered under
//! `vehicles/aircraft/<model>/…` keys, see [`toolkit::ModelAssets`]) and spawns its parts as
//! children of the vehicle body: fixed parts directly, moving parts under the pivots the
//! [`crate::visuals`] systems turn (`SurfaceVisual`, `PropellerVisual`, `WheelVisual`,
//! `PlumeVisual`, `HeatGlow`).

pub mod plane_build;
pub mod plane_paint;
pub mod plane_parts;
pub mod ship_build;
pub mod ship_gear;
pub mod ship_paint;
pub mod ship_parts;
pub mod ship_surfaces;
pub mod spaceplane;
pub mod sport_plane;
pub mod surfaces;
pub mod toolkit;

pub use plane_build::{build as build_sport_plane, spawn as spawn_sport_plane_parts};
pub use ship_build::{build as build_spaceplane, spawn as spawn_spaceplane_parts};
