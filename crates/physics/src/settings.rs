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
    /// Time one physics step may take, ms: when steps keep running over it, big simulations
    /// degrade a level at a time (bodies settling into sleep sooner, coarse level of detail
    /// nearer) until they fit, and recover when there is room again
    /// (see `PhysicsStats::load_level`). `None` = never degrade. 8 ms by default: on the RTX
    /// 5080 PC the yard's steps take 7 ms (median) without it and 3 with it (round 21d). Its
    /// levels follow `PhysicsStats::step_ms`, physics's own work (until 21a5efc7 they followed
    /// the step's span, which held other systems' time, and cost the PC 20-25% fps). They
    /// follow wall time, so tests that compare settled results turn it off.
    pub step_budget_ms: Option<f32>,
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
    /// Build static meshes and hulls of 128+ points on their own threads, placing each when it
    /// is ready or the step something moving comes near it (`crate::shape_jobs`). Off = every
    /// shape is built in the step that finds it, as before.
    pub static_meshes_off_frame: bool,
    /// Step Rapier beside the frame's drawing instead of inside its fixed step, as PhysX's
    /// `simulate` and `fetchResults` and Unreal's async physics do (`crate::overlap`): the
    /// fixed step reads the world into the simulation, the step runs on its own thread from
    /// the end of `PostUpdate` while the frame extracts, draws and ends, and its results are
    /// written back at the start of the next frame, before anything reads them. Gameplay in
    /// `FixedUpdate` sees the last step's results as before; what is drawn is half a step
    /// later at 120 FPS. Bodies are drawn between steps (`TransformInterpolation`). Off by
    /// default: headless apps and tests step inside the fixed step.
    pub overlap_frame: bool,
    /// With [`Self::overlap_frame`], a step whose Rapier part took less than this last time
    /// (ms) runs inside the fixed step instead: handing it to a thread and joining it costs
    /// more than that (a thread start and wake-up, 0.05-0.2 ms on a PC; on the 4-core cloud
    /// VM `First` waited 2.6 ms a frame at the arena for a 0.04 ms step). 0 = always beside.
    pub overlap_min_ms: f32,
}

impl Default for PhysicsSettings {
    fn default() -> Self {
        Self {
            solver_iterations: 8,
            max_substep_secs: 1.0 / 60.0,
            max_substeps: 24,
            step_budget_ms: Some(8.0),
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
            static_meshes_off_frame: true,
            overlap_frame: false,
            overlap_min_ms: 0.25,
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
