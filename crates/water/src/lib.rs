#![doc = include_str!("../README.md")]

pub mod api;
pub mod boat;
pub mod body;
pub mod buoyancy;
pub mod demo;
pub mod detail;
pub mod drops;
pub mod flow;
pub mod interact;
pub mod mirror;
pub mod puddles;
pub mod render;
pub mod ripples;
pub mod shelter;
pub mod shift;
pub mod shore;
pub mod sim;
pub mod spill;
pub mod state;
pub mod surface;
pub mod waves;

pub use boat::{Boat, HullCut, Interior, hull_collider};
pub use body::{
    RainShower, WaterBed, WaterBody, WaterConstituents, WaterExtent, WaterOptics, WaterSettings,
    WaveSettings,
};
pub use buoyancy::{Buoyancy, Floating, Tether};
pub use flow::{FlowBed, FlowFill, FlowRegion, FlowSource, Lip};
pub use interact::WaterInteractor;
pub use render::WaterFeature;
pub use shore::ShoreCover;
pub use sim::WaterLoading;
pub use state::{SharedWater, WaterState};

use std::sync::Arc;

use engine_core::math::DVec3;
use engine_core::prelude::*;
use engine_render::RenderAppExt;
use engine_world::{FocusPoints, SimulationRange, Water, WorldShiftSet};

/// Whether something `radius` meters across at `center` rests this step, by the shared
/// [`SimulationRange`]: it wakes within the active range of a focus and goes to sleep only
/// past the hysteresis band, so things near the edge don't flicker. Updates `resting`.
pub(crate) fn rests(
    range: &SimulationRange,
    focus: &FocusPoints,
    center: DVec3,
    radius: f64,
    resting: &mut bool,
) -> bool {
    if range.is_active_sphere(focus, center, radius) {
        *resting = false;
    } else if focus.distance(center) - radius.max(0.0) > range.active_m * (1.0 + range.hysteresis) {
        *resting = true;
    }
    *resting
}

/// Water: bodies with wind waves, interactive ripples, flowing regions, buoyancy, puddles, and
/// the water renderer. Installs itself as the `engine_world::Water` query provider.
pub struct WaterPlugin;

impl Plugin for WaterPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<WaterSettings>();
        let settings = app.world.resource::<WaterSettings>().clone();
        let shared = SharedWater::new(sim::init_state(&settings));
        app.insert_resource(Water(Arc::new(state::WaterProvider(shared.0.clone()))))
            .insert_resource(shared)
            .init_resource::<sim::SimBook>()
            .init_resource::<sim::RipplesInFlight>()
            .init_resource::<drops::SplashDrops>()
            .init_resource::<surface::CpuSurface>()
            .init_resource::<sim::FlowsInFlight>()
            .init_resource::<WaterLoading>()
            .init_resource::<shelter::ShelterMask>()
            .register_resource::<WaterSettings>(
                "Water quality and behavior: ripple grid size (cells, meters), ripple speed (m/s) \
                 and damping (1/s), foam life (s), raindrops per m² per mm/h, puddles on/off, \
                 wet shores on/off, mirror reflections on calm lakes on/off and their \
                 resolution.",
            )
            .register_component::<WaterBody>(
                "A body of water (ocean, lake, pool): extent around the entity, bed (terrain, \
                 flat depth or bowl), optics (absorption/scattering per meter, foam, caustics), \
                 wind-wave settings (fetch m, height scale, choppiness, swell), current (m/s). \
                 The still surface is at the entity's height.",
            )
            .register_component::<FlowRegion>(
                "Flowing water on a grid (streams, waterfalls, floods): cells, cell size (m), \
                 bed, sources (m³/s), open edges, losses (m/s), friction, basins filled at the start.",
            )
            .register_component::<Buoyancy>(
                "Floats the entity: box half extents (m), solid fill 0..1, density (kg/m³) when \
                 water moves it itself, probe grid, drag. Physics bodies get impulses instead.",
            )
            .register_component::<Floating>(
                "Velocity (m/s), spin (rad/s) and one-step push (N) of something water floats \
                 itself; how much of it is under water.",
            )
            .register_component::<Tether>(
                "Moors a floating entity: anchor (world m), rope length (m), stiffness (N/m).",
            )
            .register_component::<WaterInteractor>(
                "Makes wakes, rings and entry splashes when moving through water: body radius \
                 (m), wake strength.",
            )
            .register_component::<shore::ShoreCover>(
                "Keeps the ground under it dry of lake, stream and spray water (a jetty's deck, \
                 a boathouse): a rectangle in the entity's x/z, min and max (m).",
            )
            .register_component::<RainShower>(
                "Local rain over water and ground (shower, fountain spray, leak): radius (m), \
                 rate (mm/h). Adds rings on water and fills puddles.",
            )
            .add_systems(
                FixedUpdate,
                buoyancy::buoyancy_system.in_set(SimSet::Gameplay),
            )
            // The ripple and flow steps run beside the physics step on last step's state
            // (AGENTS.md, "Let systems run side by side"), and are put back after it, so the
            // physics step reads the same water whichever finishes first.
            .add_systems(
                FixedUpdate,
                (sim::ripple_system, sim::flow_step_system).in_set(SimSet::DuringPhysics),
            )
            .add_systems(
                FixedUpdate,
                (
                    sim::sync_bodies_system,
                    sim::publish_ripples_system,
                    surface::cpu_surface_system,
                    sim::publish_flows_system,
                    sim::flow_system,
                    sim::flow_bed_system,
                    interact::disturbance_system,
                    drops::splash_drops_system,
                    puddles::puddle_system,
                    shore::shore_wetness_system,
                    shelter::shelter_system,
                    spill::spill_audit_system,
                    sim::diagnostics_system,
                )
                    .chain()
                    .in_set(SimSet::Environment),
            )
            .add_systems(
                FixedUpdate,
                (demo::demo_system, boat::boat_air_system)
                    .chain()
                    .in_set(SimSet::Gameplay),
            )
            .add_systems(Update, boat::boat_bodies_system)
            .add_systems(Extract, mirror::lake_mirror_system)
            .add_systems(
                PreUpdate,
                (
                    demo::art::start_demo_art_system,
                    demo::art::finish_demo_art_system,
                )
                    .chain(),
            )
            .add_systems(
                PostUpdate,
                shift::shift_system.in_set(WorldShiftSet::Apply),
            )
            .add_render_feature(WaterFeature::default());
        api::register(app);
    }
}
