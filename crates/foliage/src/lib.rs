#![doc = include_str!("../README.md")]

pub mod api;
pub mod assets;
pub mod cave;
pub mod demo;
pub mod ecology;
pub mod far_woods;
pub mod felling;
pub mod grass;
pub mod grass_wear;
pub mod growth;
pub mod hand_felling;
pub mod loading;
pub mod meshgen;
pub mod near_woods;
pub mod plant;
pub mod scenery;
pub mod scenery_tree;
pub mod season;
pub mod shift;
pub mod sim;
pub mod species;
pub mod visuals;

use engine_assets::Assets;
use engine_core::prelude::*;
use engine_render::Mesh;
use engine_world::WorldShiftAppExt;

pub use api::spawn_plant;
pub use ecology::{InRegion, RegionState, RegionStats};
pub use far_woods::{FarWoods, KeepClear};
pub use felling::{ChopResult, FallingTree, FoliageCollider, LogPose, chop, fell_tree};
pub use loading::FoliageLoading;
pub use near_woods::{NearWoods, NearWoodsTree};
pub use plant::{
    Disturbance, EcologyRegion, EcologySettings, FelledLog, Genes, Plant, PlantStage,
    RegionHistory, ResourceSource,
};
pub use scenery_tree::SceneryTree;
pub use season::FoliageSeason;
pub use species::{Glow, Layer, Need, PlantForm, PlantLibrary, PlantSpecies, SpeciesId, Tolerance};

/// Foliage & ecology: procedural trees, bushes, grass, flowers and glowing mushrooms that grow,
/// compete for water, light, soil and minerals, spread their seeds, can be chopped down and
/// burnt, and grow back. See the crate docs.
pub struct FoliagePlugin;

impl Plugin for FoliagePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PlantLibrary>()
            .init_resource::<EcologySettings>()
            .init_resource::<FoliageSeason>()
            .init_resource::<ecology::GrowthClock>()
            .init_resource::<assets::FoliageVisuals>()
            .init_resource::<loading::FoliageLoading>()
            .init_resource::<visuals::GlowLights>()
            .init_resource::<FarWoods>()
            .init_resource::<far_woods::FarWoodsState>()
            .init_resource::<NearWoods>()
            .init_resource::<near_woods::NearWoodsState>()
            .init_resource::<hand_felling::HandFelling>()
            .init_resource::<hand_felling::ChoppedSpots>()
            .shift_resource::<hand_felling::ChoppedSpots>()
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
            .register_resource::<hand_felling::HandFelling>(
                "Felling by hand: grown trees within offer_radius_m of the player offer verb (E); \
                 each press is one axe blow, a grown tree takes `blows` (bigger ones more, at most \
                 max_blows). enabled = false turns it off.",
            )
            .register_resource::<EcologySettings>(
                "Ecology speed (simulated years per second), pause, and draw distances for small \
                 plants, simplified and distant trees, and scenery trees (meters).",
            )
            .register_resource::<FarWoods>(
                "Cheap 3D woods from start_m to end_m (meters from the camera) wherever the \
                 terrain's ForestCover says the land is wooded: crossed crown cards in streamed \
                 cells, spaced wider with distance. Ground under placed scenery trees is left to \
                 them; keep_clear leaves more circles alone.",
            )
            .register_resource::<NearWoods>(
                "Real scenery trees planted inside the far woods' start_m wherever the terrain's \
                 ForestCover says the land is wooded (spacing_m apart in a closed wood, in \
                 streamed cells of cell_m), so walking into painted forest finds trees all round.",
            )
            .register_resource::<FoliageSeason>(
                "The time of year plants show (day_of_year, 100 ≈ 10 April, 290 ≈ 17 October; \
                 midsummer by default), or follow_weather to take the weather's calendar: \
                 deciduous trees leaf out, turn and drop their leaves, ferns and flowers die back, \
                 grass turns to straw.",
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
            .register_component::<cave::CaveChamber>(
                "A cave's rock chamber seen from inside (origin = floor center): sculpted walls and \
                 a vaulted roof with stalactites and stalagmites, open at the floor and at a mouth \
                 in the -Z wall; moss and lichen only in the mouth's daylight.",
            )
            .register_component::<SceneryTree>(
                "A static tree with a species' looks for painted landscapes (whole valleys of \
                 them): no growth, seeding, fire or felling and no collider; it draws near, far \
                 or distant detail by camera distance and follows the season.",
            )
            .register_component::<growth::GrowingPlant>(
                "A plant grown from a growth graph (preset strawberry or sapling, or graph: a \
                 PlantGraph of stems, leaves, flowers and fruit generators), aged age_days and \
                 moving on days_per_second (a time-lapse); repeat starts it again from seed.",
            )
            .register_component::<scenery::LitterPiece>(
                "Scattered twig, branch, cone, leaves or reeds; hidden beyond draw_distance_m.",
            )
            .register_component::<demo::DemoFelling>(
                "Showcase: fells the nearest mature tree in a ring around it every every_secs seconds.",
            );
        engine_render::RenderAppExt::add_render_feature(app, scenery::SceneryFeature::default());
        // Unless rendering is off, the renderer wants the ground layers on its first frame.
        if !app
            .world
            .contains_resource::<engine_render::RenderDisabled>()
        {
            scenery::start_painting_ground_layers();
        }
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
                    felling::release_grass_presses,
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
            (
                shift::follow_world_shift,
                far_woods::follow_world_shift,
                near_woods::follow_world_shift,
            )
                .in_set(engine_world::WorldShiftSet::Apply),
        )
        .add_systems(
            Update,
            (
                (felling::stream_tree_colliders, felling::sync_tree_colliders).chain(),
                season::follow_weather_calendar,
                (grass::sync_region_grass, grass_wear::wear_grass).chain(),
                felling::remove_orphan_colliders,
                ecology::drop_unloaded_regions,
                scenery::drop_unloaded_covers,
                (hand_felling::chop_on_interact, hand_felling::offer_chops).chain(),
                cave::sync_chamber_colliders,
                (
                    scenery::build_scenery,
                    scenery::cull_debris,
                    assets::ensure_species_visuals,
                    season::apply_season_colors,
                    season::apply_litter_color,
                    visuals::sync_plant_visuals,
                    growth::grow_plants,
                    scenery_tree::sync_scenery_trees,
                    far_woods::stream_far_woods,
                    near_woods::stream_near_woods,
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
