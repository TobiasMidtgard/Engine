//! Smoke tuning.

use engine_core::prelude::*;

use crate::domain::StepParams;

/// Smoke tuning, live through the AI bridge (`smoke.*`, or set the resource).
#[derive(Resource, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SmokeSettings {
    pub enabled: bool,
    /// Simulation steps per second of world time.
    pub sim_hz: f32,
    /// Multiplier on physical buoyancy (hot smoke rising).
    pub buoyancy: f32,
    /// Downward pull per kg/m³ of smoke, m/s².
    pub weight: f32,
    /// How fast smoke takes up the wind, 1/s.
    pub wind_coupling: f32,
    /// Share of the wind coupling on vertical motion.
    pub vertical_coupling: f32,
    /// Swirl (vorticity confinement), 0 = smooth.
    pub vorticity: f32,
    /// Pressure solver iterations (more = rounder, costlier flow).
    pub iterations: u32,
    /// Seconds for smoke to thin to 1/e by itself.
    pub dissipation_secs: f32,
    /// Seconds for hot smoke to cool to 1/e.
    pub cool_secs: f32,
    /// Mixing too small for the voxels, m²/s: softer, wider clouds.
    pub diffusion_m2_s: f32,
    /// Light blocked per kg of smoke, m²/kg (soot ≈ 8 700, grenade smoke ≈ 3 000).
    pub extinction_m2_kg: f32,
    /// Voxel size of volumes created around smoke released outside any `SmokeVolume`, m.
    pub auto_cell_m: f32,
    /// Size of those volumes, m (x, y, z).
    pub auto_size_m: [f32; 3],
    /// Most smoke volumes at once.
    pub max_volumes: u32,
    /// Volumes farther than this from every focus point (or camera) pause.
    pub sim_radius_m: f32,
    /// How much of the smoke a bullet clears along its path (0..1).
    pub bullet_clear: f32,
    /// Radius of the clear tunnel a bullet leaves, m.
    pub tunnel_radius_m: f32,
    /// Seconds the sharp tunnel stays visible while the smoke refills it.
    pub tunnel_secs: f32,
    /// Fine billowing detail added when drawing (0 = smooth voxels).
    pub detail: f32,
    /// Most raymarch steps per pixel.
    pub max_steps: u32,
}

impl Default for SmokeSettings {
    fn default() -> Self {
        let p = StepParams::default();
        Self {
            enabled: true,
            sim_hz: 20.0,
            buoyancy: p.buoyancy,
            weight: p.weight,
            wind_coupling: p.wind_coupling,
            vertical_coupling: p.vertical_coupling,
            vorticity: p.vorticity,
            iterations: p.iterations,
            dissipation_secs: p.dissipation_secs,
            cool_secs: p.cool_secs,
            diffusion_m2_s: p.diffusion_m2_s,
            extinction_m2_kg: 3000.0,
            auto_cell_m: 1.0,
            auto_size_m: [48.0, 40.0, 48.0],
            max_volumes: 8,
            sim_radius_m: 1500.0,
            bullet_clear: 0.99,
            tunnel_radius_m: 0.18,
            tunnel_secs: 1.6,
            detail: 1.0,
            max_steps: 96,
        }
    }
}

impl SmokeSettings {
    pub fn step_params(&self, dt: f32, ambient_c: f32) -> StepParams {
        StepParams {
            dt,
            ambient_c,
            buoyancy: self.buoyancy,
            weight: self.weight,
            wind_coupling: self.wind_coupling,
            vertical_coupling: self.vertical_coupling,
            vorticity: self.vorticity,
            iterations: self.iterations.clamp(1, 100),
            dissipation_secs: self.dissipation_secs,
            cool_secs: self.cool_secs,
            diffusion_m2_s: self.diffusion_m2_s.max(0.0),
        }
    }
}
