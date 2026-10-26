//! Tunable physics settings (a registered resource, so data files and AI agents can edit it).

use engine_core::prelude::*;
use rapier3d::prelude::IntegrationParameters;

/// How the physics module simulates. Edit at runtime through `resource.patch` or code.
#[derive(Resource, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PhysicsSettings {
    /// Solver iterations per substep. Rapier's iterations are themselves small steps (it
    /// divides the substep among them, finding contacts once), so these are where precision
    /// comes from: 8 at 60 Hz solves as finely as 4 at 120 Hz for about half the cost, as in
    /// Box2D's "soft step" and Jolt (`engine-design/physics-perf-research.md`). More = stiffer
    /// stacks and joints, slower.
    pub solver_iterations: u32,
    /// Longest world-time substep, seconds. Each fixed step is split into substeps no longer
    /// than this (one per 60 Hz step by default), so fast-forward (time scale > 1) is as stable
    /// as 1×. Each substep finds contacts anew and pays for every contact's bookkeeping, so
    /// prefer more `solver_iterations` to shorter substeps.
    pub max_substep_secs: f32,
    /// Upper bound on substeps per fixed step (protects the frame rate at extreme speed-ups).
    pub max_substeps: u32,
    /// How far colliders may sink into each other before being pushed apart, meters. Smaller
    /// = tall stacks sag less, but resting contacts need more work.
    pub allowed_penetration_m: f32,
    /// Stiffness of contacts as a spring frequency, Hz (contacts with static geometry use
    /// twice this). Higher = less give under heavy loads; too high for the substep rate
    /// makes stacks jitter.
    pub contact_stiffness_hz: f32,
    /// Bodies stay within this distance of their region's anchor, meters (see the crate docs on
    /// regions). Smaller = more precise far from the origin; larger = fewer regions.
    pub region_radius_m: f64,
    /// Regions farther than this from every `SimulationFocus` pause (bodies there keep their
    /// state). `None` = simulate everywhere. Has no effect when there is no focus.
    pub freeze_distance_m: Option<f64>,
    /// Level of detail: bodies farther than this from every `SimulationFocus` simulate with
    /// substeps twice as long, meters. A pixel there spans 15 cm (1080p, 60° field of view),
    /// more than the difference. Busy far piles split off
    /// into regions of their own for it, and merge back when they come near. `None` = full
    /// detail everywhere. Has no effect when there is no focus.
    pub coarse_beyond_m: Option<f64>,
    /// Farther than this (and past `coarse_beyond_m`), bodies step every other frame with
    /// substeps four times as long, and are drawn moving on at their velocity in between,
    /// meters. Regions with characters stay coarse. `None` = never at half rate.
    pub half_rate_beyond_m: Option<f64>,
    /// Rolling resistance coefficient on hard ground: how much balls, barrels, wheels and
    /// capsules slow down rolling (≈ 0.01 for rubber on concrete; 0 = roll forever). Mud, sand,
    /// snow and grass add more, from the surface's deformation.
    pub rolling_resistance: f32,
    /// Fastest a body may move, m/s (keeps a solver blow-up from flinging bodies to infinity).
    /// Spaceships can go far faster than cars, so the default is high.
    pub max_speed: f32,
    /// Send `ContactEvent`s.
    pub contact_events: bool,
    /// Turn hard impacts into `GroundContact` (dents, dust, sounds) and `NoiseEvent` (AI hears
    /// it) messages.
    pub impact_events: bool,
    /// Smallest change of speed that counts as an audible impact, m/s.
    pub impact_min_speed: f32,
    /// Bodies float and drag in water (uses the `Water` query).
    pub buoyancy: bool,
    /// Explosions (`WorldImpulse`) push dynamic bodies.
    pub blast_response: bool,
    /// Multiplies how hard explosions push. 1 = real blast wind (a grenade barely moves a
    /// tonne of stone); action games often use 3–10.
    pub blast_scale: f32,
    /// The joints of a free-floating group of dynamic bodies (ragdolls, loose chains) become
    /// articulations: each body only moves along its joint's free axes, so joints never open
    /// however hard the landing. Groups pinned to the world or a kinematic body, or with a body
    /// over 100× heavier than another, keep constraint joints (Rapier's articulations blow up
    /// there). Costs about 1 ms per tumbling ragdoll per frame. Off = every joint is a
    /// constraint the solver pulls together each step.
    pub articulated_joints: bool,
    /// Most moving free-floating jointed groups (ragdolls, loose chains) solved as articulations
    /// at once: the ones nearest a `SimulationFocus`. The rest use constraint joints until a
    /// place frees up (a tumbling ragdoll costs about 0.9 ms per frame articulated, 0.15 ms on
    /// constraint joints, where its joints may open a few centimeters for a frame on hard
    /// landings). Sleeping groups keep their joints and don't count.
    pub max_articulated: u32,
    /// Only jointed groups this close to a `SimulationFocus` are articulated, meters (one
    /// already articulated keeps it until a quarter farther). Past 20 m the few centimeters a
    /// constraint joint opens on a hard landing are about a pixel at 1080p, so the cost buys
    /// nothing anyone sees. Infinite = only `max_articulated` limits it.
    pub articulate_within_m: f32,
}

impl Default for PhysicsSettings {
    fn default() -> Self {
        Self {
            solver_iterations: 8,
            max_substep_secs: 1.0 / 60.0,
            max_substeps: 24,
            allowed_penetration_m: 0.001,
            contact_stiffness_hz: 60.0,
            region_radius_m: 1000.0,
            freeze_distance_m: Some(25_000.0),
            coarse_beyond_m: Some(150.0),
            half_rate_beyond_m: Some(600.0),
            rolling_resistance: 0.01,
            max_speed: 100_000.0,
            contact_events: true,
            impact_events: true,
            impact_min_speed: 1.5,
            buoyancy: true,
            blast_response: true,
            blast_scale: 1.0,
            articulated_joints: true,
            max_articulated: 4,
            articulate_within_m: 20.0,
        }
    }
}

impl PhysicsSettings {
    /// Copy the solver settings into a Rapier world's parameters.
    pub(crate) fn apply_to(&self, p: &mut IntegrationParameters) {
        p.num_solver_iterations = self.solver_iterations.clamp(1, 64) as usize;
        p.normalized_max_linear_velocity = self.max_speed.max(1.0);
        p.normalized_allowed_linear_error = self.allowed_penetration_m.clamp(0.0, 0.1);
        let hz = self.contact_stiffness_hz.max(1.0);
        p.contact_softness.natural_frequency = hz;
        p.static_contact_softness.natural_frequency = hz * 2.0;
        // Joints (ragdolls, chains) are much steadier with warm starting.
        p.warmstart_joints = true;
    }
}
