#![doc = include_str!("../README.md")]

mod api;
mod articulation;
mod buoyancy;
mod capsule_box;
mod character;
mod cones;
mod convert;
mod debug;
pub mod demo;
mod fine;
mod lod;
mod queries;
mod report;
mod rolling;
mod settings;
mod shift;
mod sim;
mod step;

use std::sync::Arc;

use engine_core::prelude::*;
use engine_world::physics::{self as contract, PhysicsQueries};
use engine_world::{GroundContact, NoiseEvent, WorldImpulse};
use parking_lot::RwLock;

pub use debug::PhysicsDebug;
pub use fine::FineSteps;
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
    /// Joints solved as articulations (see [`PhysicsSettings::articulated_joints`]).
    pub articulated: usize,
    /// Static colliders copied into regions, all regions together (a static shared by two
    /// regions counts twice).
    pub static_copies: usize,
    /// Awake bodies simulated coarsely, far from every focus (see
    /// [`PhysicsSettings::coarse_beyond_m`]).
    pub coarse_awake: usize,
    /// Awake bodies simulated at half rate, farther still (see
    /// [`PhysicsSettings::half_rate_beyond_m`]).
    pub half_rate_awake: usize,
    /// Awake bodies in regions stepping in fine substeps (see [`FineSteps`]).
    pub fine_awake: usize,
}

impl PhysicsStats {
    fn from_sim(sim: &sim::Simulation, step_ms: f64, substeps: u32) -> Self {
        let (mut by_tier, mut fine_awake) = ([0usize; 3], 0);
        for (_, r) in sim.live_regions() {
            let awake = r
                .world
                .bodies
                .iter()
                .filter(|(_, b)| !b.is_fixed() && !b.is_sleeping())
                .count();
            by_tier[r.tier.min(2) as usize] += awake;
            if r.fine.is_some() {
                fine_awake += awake;
            }
        }
        let awake = by_tier.iter().sum();
        Self {
            bodies: sim.bodies.len() + sim.characters.len(),
            awake,
            regions: sim.live_regions().count(),
            touching: sim.contacts.touching.len(),
            step_ms,
            substeps,
            articulated: sim
                .joints
                .values()
                .filter(|j| matches!(j.created, Some((_, sim::JointHandle::Articulated { .. }))))
                .count(),
            static_copies: sim.live_regions().map(|(_, r)| r.statics.len()).sum(),
            coarse_awake: by_tier[1],
            half_rate_awake: by_tier[2],
            fine_awake,
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
            .init_resource::<PhysicsDebug>()
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
            .register_resource::<PhysicsDebug>(
                "Physics debug drawing (needs DebugPlugin): colliders, contacts, joints, \
                 regions, centers_of_mass, velocities (bools), radius_m around the focus, on_top.",
            )
            .register_resource::<PhysicsStats>(
                "Last physics step (read-only): bodies, awake bodies, regions, touching pairs, \
                 step time (ms), substeps.",
            )
            .add_systems(FixedUpdate, step::physics_step.in_set(SimSet::Physics))
            .add_systems(FixedUpdate, demo::replay_stations.in_set(SimSet::Gameplay))
            .add_systems(
                FixedUpdate,
                demo::jenga::drive_rods
                    .in_set(SimSet::Gameplay)
                    .after(demo::replay_stations),
            )
            .add_systems(FixedUpdate, demo::replay_floaters.in_set(SimSet::Gameplay))
            .add_systems(
                PostUpdate,
                shift::shift_simulation.in_set(engine_world::WorldShiftSet::Apply),
            )
            .add_systems(PostUpdate, debug::draw_physics_debug)
            .add_systems(Last, step::note_removals);
        api::register(app);
    }
}
