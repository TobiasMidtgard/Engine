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
pub mod fit;
mod grounding;
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
pub use fit::{Fit, FitCollider};
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
    /// Bodies not asleep in regions resting past the world's `SimulationRange`: held where
    /// they are, not stepping, until a focus comes back within range.
    pub resting_awake: usize,
    /// Bodies in dormant regions: asleep and apart from everything awake, so they sit steps
    /// out though a region near them keeps stepping.
    pub dormant: usize,
    /// Bodies held asleep until the ground under them loads (past the simulation range, with
    /// no collider under them yet).
    pub ungrounded: usize,
}

impl PhysicsStats {
    fn from_sim(sim: &sim::Simulation, step_ms: f64, substeps: u32) -> Self {
        let (mut by_tier, mut fine_awake, mut dormant) = ([0usize; 4], 0, 0);
        for (_, r) in sim.live_regions() {
            if r.dormant {
                dormant += r.world.bodies.iter().filter(|(_, b)| !b.is_fixed()).count();
            }
            let awake = r
                .world
                .bodies
                .iter()
                .filter(|(_, b)| !b.is_fixed() && !b.is_sleeping())
                .count();
            by_tier[r.tier.min(lod::REST) as usize] += awake;
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
            resting_awake: by_tier[3],
            dormant,
            ungrounded: sim.ungrounded.len(),
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
            .init_resource::<step::StepCarry>()
            // Reading the world and writing it back take the whole world; Rapier's part
            // between them touches only the simulation, so `SimSet::DuringPhysics` runs
            // beside it. It starts once the world is read (as Unreal's TG_DuringPhysics
            // starts once the physics step is kicked off): begun first, it would hold the
            // read back, and Rapier would run after it, alone.
            .add_systems(
                FixedUpdate,
                (
                    step::physics_sync.before(SimSet::DuringPhysics),
                    step::physics_simulate,
                    step::physics_write_back,
                )
                    .chain()
                    .in_set(SimSet::Physics),
            )
            .init_resource::<demo::StationWork>()
            .init_resource::<demo::FloatWork>()
            // The replays take the whole world (they spawn and despawn stations), so they
            // run only on the steps that have work: an exclusive system stops every other
            // system of the step. The cycles tick in plain systems beside the others.
            .add_systems(
                FixedUpdate,
                (
                    demo::tick_stations,
                    demo::replay_stations.run_if(demo::stations_have_work),
                )
                    .chain()
                    .in_set(SimSet::Gameplay),
            )
            .add_systems(
                FixedUpdate,
                demo::jenga::drive_rods
                    .in_set(SimSet::Gameplay)
                    .after(demo::replay_stations),
            )
            .add_systems(
                FixedUpdate,
                (
                    demo::tick_floaters,
                    demo::replay_floaters.run_if(demo::floaters_have_work),
                )
                    .chain()
                    .in_set(SimSet::Gameplay),
            )
            .add_systems(Update, demo::finish_art.run_if(demo::art_busy))
            .add_systems(Update, fit::fit_colliders.run_if(fit::fits_waiting))
            .add_systems(
                PostUpdate,
                (shift::shift_simulation, demo::follow_world_shift)
                    .in_set(engine_world::WorldShiftSet::Apply),
            )
            .add_systems(PostUpdate, debug::draw_physics_debug)
            .add_systems(Last, step::note_removals);
        api::register(app);
    }
}
