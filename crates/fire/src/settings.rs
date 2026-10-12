//! Tunables for the fire module (one resource, editable at runtime by name: `FireSettings`).

use engine_core::prelude::*;

use crate::field::SpreadParams;

/// Fire simulation and look. Every field can be changed while the game runs
/// (`resource.patch FireSettings {...}`), except `cell_m`, which applies to new games.
#[derive(Resource, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FireSettings {
    pub enabled: bool,
    /// Ground cell size, meters. Smaller is finer and costs more (set before startup).
    pub cell_m: f32,
    /// Simulation steps per second of world time (slow motion slows the fire with the world).
    pub sim_hz: f32,
    /// Fire only simulates within this distance of a `SimulationFocus` (or the camera when
    /// there is none), meters. Farther fires pause until someone comes back.
    pub sim_radius_m: f32,
    /// Whether ground surface materials (grass, leaves, wood) are fuel. Turn off to burn only
    /// inside `FuelPatch`es.
    pub ground_fuel: bool,
    /// Share of a burning cell's heat that preheats its neighbors.
    pub transfer: f32,
    /// Decay length of heat transfer in still air, meters.
    pub reach_m: f32,
    /// How much farther heat reaches downwind, per m/s of wind.
    pub wind_reach: f32,
    /// How much more heat goes downwind, per m/s of wind.
    pub wind_boost: f32,
    /// How quickly spread against the wind dies off, per m/s of wind.
    pub upwind_damping: f32,
    /// Share of absorbed heat lost to the air per second (a weak fire stops at gaps).
    pub cooling: f32,
    /// Throw embers that drift downwind and can start spot fires.
    pub embers: bool,
    /// Multiplier on ember production.
    pub ember_rate: f32,
    pub max_embers: u32,
    /// Chance (0..1) that a hot ember landing on dry fuel starts a fire.
    pub spotting: f32,
    /// Damage per second to things standing in full flames (`DamageEvent`, kind Fire).
    pub damage_per_second: f32,
    /// Most point lights fires add to the scene.
    pub max_lights: u32,
    /// Light output per kW of heat release, lumens.
    pub light_lm_per_kw: f32,
    /// Smoke mass per kg of fuel burned (sent as `SmokeEmission`).
    pub smoke_yield: f32,
    /// Most fire tiles (16 m squares) kept in memory.
    pub max_tiles: u32,
    /// Flames and scorch are drawn within this distance of the camera, meters.
    pub view_distance_m: f32,
}

impl Default for FireSettings {
    fn default() -> Self {
        let s = SpreadParams::default();
        Self {
            enabled: true,
            cell_m: 0.5,
            sim_hz: 20.0,
            sim_radius_m: 800.0,
            ground_fuel: true,
            transfer: s.transfer,
            reach_m: s.reach_m,
            wind_reach: s.wind_reach,
            wind_boost: s.wind_boost,
            upwind_damping: s.upwind_damping,
            cooling: s.cooling,
            embers: true,
            ember_rate: 1.0,
            max_embers: 3000,
            spotting: 0.08,
            damage_per_second: 25.0,
            max_lights: 8,
            light_lm_per_kw: 30.0,
            smoke_yield: 0.06,
            max_tiles: 4096,
            view_distance_m: 1500.0,
        }
    }
}

impl FireSettings {
    pub fn spread(&self) -> SpreadParams {
        SpreadParams {
            transfer: self.transfer,
            reach_m: self.reach_m,
            wind_reach: self.wind_reach,
            wind_boost: self.wind_boost,
            upwind_damping: self.upwind_damping,
            cooling: self.cooling,
        }
    }
}
