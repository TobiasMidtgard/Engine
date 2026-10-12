#![doc = include_str!("../README.md")]

mod api;
mod character;
mod convert;
pub mod demo;
mod queries;
mod settings;
mod shift;
mod sim;
mod step;

use std::sync::Arc;

use engine_core::prelude::*;
use engine_world::physics::{self as contract, PhysicsQueries};
use engine_world::{GroundContact, NoiseEvent, WorldImpulse};
use parking_lot::RwLock;

pub use settings::PhysicsSettings;

/// Rigid-body physics on Rapier: bodies, colliders, joints, characters and scene queries,
/// behind the `engine_world::physics` contract. Add exactly one physics plugin.
pub struct PhysicsPlugin;

/// The simulation, shared between the step (writer) and [`PhysicsQueries`] (readers).
#[derive(Resource, Clone)]
pub(crate) struct SharedSimulation(pub(crate) Arc<RwLock<sim::Simulation>>);

/// What the last physics step did (read-only; also in `Diagnostics` as `physics.*`).
#[derive(Resource, Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PhysicsStats {
    /// Dynamic and kinematic bodies plus characters.
    pub bodies: usize,
    /// Bodies that are moving (not asleep).
    pub awake: usize,
    /// Rapier worlds in use (see the crate docs on regions).
    pub regions: usize,
    /// Collider pairs touching.
    pub touching: usize,
    /// Time the last step took, milliseconds.
    pub step_ms: f64,
    /// Substeps in the last fixed step.
    pub substeps: u32,
}

impl PhysicsStats {
    fn from_sim(sim: &sim::Simulation, step_ms: f64, substeps: u32) -> Self {
        let awake = sim
            .live_regions()
            .map(|(_, r)| {
                r.world
                    .bodies
                    .iter()
                    .filter(|(_, b)| !b.is_fixed() && !b.is_sleeping())
                    .count()
            })
            .sum();
        Self {
            bodies: sim.bodies.len() + sim.characters.len(),
            awake,
            regions: sim.live_regions().count(),
            touching: sim.contacts.touching.len(),
            step_ms,
            substeps,
        }
    }
}

impl Plugin for PhysicsPlugin {
    fn build(&self, app: &mut App) {
        contract::claim_backend(app, "engine_physics (rapier3d)");
        let shared = Arc::new(RwLock::new(sim::Simulation::default()));
        app.insert_resource(SharedSimulation(shared.clone()))
            .insert_resource(PhysicsQueries(Arc::new(queries::RapierQueries(shared))))
            .init_resource::<PhysicsSettings>()
            .init_resource::<PhysicsStats>()
            .init_resource::<step::PendingRemovals>()
            // Normally added by `WorldPlugin`; repeated so plugin order doesn't matter.
            .init_resource::<engine_world::Gravity>()
            .init_resource::<engine_world::SurfaceMaterials>()
            .init_resource::<engine_world::FocusPoints>()
            .add_message::<contract::ApplyImpulse>()
            .add_message::<contract::ContactEvent>()
            .add_message::<WorldImpulse>()
            .add_message::<GroundContact>()
            .add_message::<NoiseEvent>()
            .add_message::<engine_world::WorldShift>()
            .register_resource::<PhysicsSettings>(
                "Physics tuning: solver iterations, substep length (s), region radius (m), \
                 freeze distance (m), max speed (m/s), contact/impact events, buoyancy, blasts.",
            )
            .register_resource::<PhysicsStats>(
                "Last physics step (read-only): bodies, awake bodies, regions, touching pairs, \
                 step time (ms), substeps.",
            )
            .add_systems(FixedUpdate, step::physics_step.in_set(SimSet::Physics))
            .add_systems(
                PostUpdate,
                shift::shift_simulation.in_set(engine_world::WorldShiftSet::Apply),
            )
            .add_systems(Last, step::note_removals);
        api::register(app);
    }
}
