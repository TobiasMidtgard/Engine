#![doc = include_str!("../README.md")]

pub mod api;
pub mod assets;
pub mod demo;
pub mod ecology;
pub mod felling;
pub mod meshgen;
pub mod plant;
pub mod scenery;
pub mod shift;
pub mod sim;
pub mod species;
pub mod visuals;

use engine_assets::Assets;
use engine_core::prelude::*;
use engine_render::Mesh;

pub use api::spawn_plant;
pub use ecology::{InRegion, RegionState, RegionStats};
pub use felling::{ChopResult, FallingTree, FoliageCollider, LogPose, chop, fell_tree};
pub use plant::{
    Disturbance, EcologyRegion, EcologySettings, FelledLog, Genes, Plant, PlantStage,
    RegionHistory, ResourceSource,
};
pub use species::{Glow, Layer, Need, PlantForm, PlantLibrary, PlantSpecies, SpeciesId, Tolerance};

/// Foliage & ecology: procedural trees, bushes, grass, flowers and glowing mushrooms that grow,
/// compete for water, light, soil and minerals, spread their seeds, can be chopped down and
/// burnt, and grow back. See the crate docs.
pub struct FoliagePlugin;

impl Plugin for FoliagePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PlantLibrary>()
            .init_resource::<EcologySettings>()
            .init_resource::<ecology::GrowthClock>()
            .init_resource::<assets::FoliageVisuals>()
            .init_resource::<visuals::GlowLights>()
            .add_message::<engine_world::DamageEvent>()
            .add_message::<engine_world::WorldImpulse>()
            .add_message::<engine_world::NoiseEvent>()
            .add_message::<engine_world::GroundContact>()
            .add_message::<engine_world::TerrainChanged>()
            .register_resource::<PlantLibrary>(
                "Plant species: looks, size, growth speed, needs (water, light, nutrients, minerals, \
                 temperature as tolerance curves), seeding, glow, fuel and toughness. Edit to tune a \
                 species live; add new ones with foliage.define_species.",
            )
            .register_resource::<EcologySettings>(
                "Ecology speed (simulated years per second), pause, and draw distances for small \
                 plants and simplified trees (meters).",
            )
            .register_component::<Plant>(
                "A plant: species, size 0..1, age (years), health, vigor, what limits it, stage \
                 (alive, stump, dead, burnt), genes and chop damage. Spawn with foliage.plant.",
            )
            .register_component::<EcologyRegion>(
                "An area where plants grow, compete and spread on their own: size (meters), species \
                 that seed in, sky light, base soil moisture/nutrients/minerals, history to prewarm \
                 (grow years, clear-cut or burn), time scale.",
            )
            .register_component::<ResourceSource>(
                "Adds water, nutrients or minerals to the soil around it (a spring, a mineral pool, \
                 compost; negative for a dry ridge), fading to zero at radius_m meters.",
            )
            .register_component::<FelledLog>(
                "A fallen tree lying on the ground; rots away over rot_years, feeding the soil.",
            )
            .register_component::<LogPose>("Which way a log fell (yaw, radians) and the tree's genes.")
            .register_component::<demo::GroundCover>(
                "Forest or cave floor laid over the ground, textured from the soil and plants on it \
                 (leaf litter, needles, moss where wet, ash where burnt, packed earth on the trail, \
                 mud round pools, mineral crust where mineral water soaks in), with debris scattered.",
            )
            .register_component::<scenery::Rock>(
                "A procedural boulder, stone or pebble (scale = size in meters), sunk bury_m into the \
                 ground cover, with moss on top, lichen and a wet line below water_m.",
            )
            .register_component::<scenery::Debris>(
                "Scattered twig, branch, cone, leaves or reeds; hidden beyond draw_distance_m.",
            )
            .register_component::<demo::DemoFelling>(
                "Showcase: fells the nearest mature tree in a ring around it every every_secs seconds.",
            );
        engine_render::RenderAppExt::add_render_feature(app, scenery::SceneryFeature::default());
        api::register(app);

        app.add_systems(
            PreUpdate,
            (
                api::apply_new_sources,
                ecology::init_regions,
                ecology::assign_regions,
            )
                .chain(),
        )
        .add_systems(
            FixedUpdate,
            (
                demo::run_demo_felling.in_set(SimSet::Gameplay),
                (
                    felling::apply_damage,
                    felling::update_falling_trees,
                    felling::place_logs,
                )
                    .chain()
                    .in_set(SimSet::PostPhysics),
                (
                    ecology::react_to_fire,
                    ecology::scorch_small_plants,
                    ecology::trample,
                    ecology::follow_terrain,
                    ecology::grow_plants,
                    ecology::ecology_step,
                )
                    .chain()
                    .in_set(SimSet::Environment),
            ),
        )
        .add_systems(
            PostUpdate,
            shift::follow_world_shift.in_set(engine_world::WorldShiftSet::Apply),
        )
        .add_systems(
            Update,
            (
                (felling::stream_tree_colliders, felling::sync_tree_colliders).chain(),
                felling::remove_orphan_colliders,
                (
                    scenery::build_scenery,
                    scenery::cull_debris,
                    assets::ensure_species_visuals,
                    visuals::sync_plant_visuals,
                    visuals::sync_log_visuals,
                    visuals::update_glow_lights,
                    visuals::pulse_glow,
                )
                    .chain()
                    .run_if(resource_exists::<Assets<Mesh>>),
            )
                .in_set(UpdateSet::Logic),
        );
    }
}
