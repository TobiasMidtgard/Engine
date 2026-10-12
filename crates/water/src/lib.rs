#![doc = include_str!("../README.md")]

pub mod api;
pub mod body;
pub mod buoyancy;
pub mod demo;
pub mod flow;
pub mod interact;
pub mod puddles;
pub mod render;
pub mod ripples;
pub mod sim;
pub mod state;
pub mod waves;

pub use body::{
    RainShower, WaterBed, WaterBody, WaterExtent, WaterOptics, WaterSettings, WaveSettings,
};
pub use buoyancy::{Buoyancy, Floating, Tether};
pub use flow::{FlowBed, FlowFill, FlowRegion, FlowSource};
pub use interact::WaterInteractor;
pub use render::WaterFeature;
pub use state::{SharedWater, WaterState};

use std::sync::Arc;

use engine_core::prelude::*;
use engine_render::RenderAppExt;
use engine_world::Water;

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
            .register_resource::<WaterSettings>(
                "Water quality and behavior: ripple grid size (cells, meters), ripple speed (m/s) \
                 and damping (1/s), foam life (s), raindrops per m² per mm/h, puddles on/off.",
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
            .register_component::<RainShower>(
                "Local rain over water and ground (shower, fountain spray, leak): radius (m), \
                 rate (mm/h). Adds rings on water and fills puddles.",
            )
            .add_systems(
                FixedUpdate,
                buoyancy::buoyancy_system.in_set(SimSet::Gameplay),
            )
            .add_systems(
                FixedUpdate,
                (
                    sim::sync_bodies_system,
                    interact::disturbance_system,
                    sim::ripple_system,
                    sim::flow_system,
                    puddles::puddle_system,
                    sim::diagnostics_system,
                )
                    .chain()
                    .in_set(SimSet::Environment),
            )
            .add_systems(FixedUpdate, demo::demo_system.in_set(SimSet::Gameplay))
            .add_render_feature(WaterFeature::default());
        api::register(app);
    }
}
