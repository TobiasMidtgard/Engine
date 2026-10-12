#![doc = include_str!("../README.md")]

use engine_core::prelude::*;

/// Smoke, gas & volumetric clouds. Stub: add it now so games compile against the final plugin name.
pub struct SmokePlugin;

impl Plugin for SmokePlugin {
    fn build(&self, _app: &mut App) {}
}
