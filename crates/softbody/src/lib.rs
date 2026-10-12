#![doc = include_str!("../README.md")]

use engine_core::prelude::*;

/// Cloth, ropes and soft bodies. Stub: add it now so games compile against the final plugin name.
pub struct SoftBodyPlugin;

impl Plugin for SoftBodyPlugin {
    fn build(&self, _app: &mut App) {}
}
