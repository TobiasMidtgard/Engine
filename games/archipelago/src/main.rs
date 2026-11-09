//! Archipelago: a city-building and trade game on a chain of islands, the engine's test of
//! many buildings, carts, ships and aircraft at once (see `engine-design/anno-like-game.md`).
//!
//! ```sh
//! cargo run -p archipelago --release                          # play
//! cargo run -p archipelago --release -- --headless --tour     # writes screenshots/archipelago/*.png
//! cargo run -p archipelago --release -- --headless --tour town,harbour
//! ```
//!
//! Keys: WASD or arrows pan (Shift faster), Q/E turn, wheel zooms, right-drag pans, middle-drag
//! turns. Pick a building in the bar at the bottom, R turns it, left click places it, right
//! click or Esc cancels. 1/2/3 set the game speed, Space pauses.

mod api;
mod art;
mod build;
mod camera;
mod defs;
mod draw;
mod effects;
mod fields;
mod fleet;
mod grid;
mod hud;
mod icons;
mod island;
mod logistics;
mod looks;
mod orders;
mod sim;
mod streets;
mod tools;
mod tour;
mod town;
mod vessels;
mod walkers;
mod world;

use engine::prelude::*;

fn main() {
    let mut app = App::new();
    engine::core::log::init("info,wgpu=warn,naga=warn");
    app.world.resource_mut::<EngineConfig>().app_name = "Archipelago".into();
    engine::apply_cli_args(&mut app);
    let args: Vec<String> = std::env::args().collect();
    let tour = args
        .iter()
        .position(|a| a == "--tour")
        .map(|i| args.get(i + 1).filter(|s| !s.starts_with("--")).cloned());
    if let Some(i) = args.iter().position(|a| a == "--woods") {
        let d = args.get(i + 1).and_then(|s| s.parse().ok()).unwrap_or(1.0);
        app.insert_resource(world::WoodsDensity(d));
    }
    add_game(&mut app);
    match tour {
        Some(only) => tour::run(app, only.as_deref()),
        None => {
            app.run();
        }
    }
}

/// Everything the game adds to an app.
pub fn add_game(app: &mut App) {
    app.add_plugins(DefaultPlugins)
        .add_plugins((
            engine_terrain::TerrainPlugin,
            engine_water::WaterPlugin,
            engine_weather::WeatherPlugin,
            engine_foliage::FoliagePlugin,
            engine_ui::UiPlugin,
            engine_vfx::VfxPlugin,
        ))
        .init_resource::<grid::Grid>()
        .init_resource::<sim::Economy>()
        .init_resource::<sim::Shipments>()
        .init_resource::<build::Changed>()
        .init_resource::<build::Forest>()
        .init_resource::<draw::Drawn>()
        .init_resource::<streets::Streets>()
        .init_resource::<walkers::Walkers>()
        .init_resource::<fleet::SeaChart>()
        .init_resource::<fleet::Routes>()
        .init_resource::<tools::Tool>()
        .add_systems(Startup, setup)
        .add_systems(FixedUpdate, sim::economy_system)
        .add_systems(
            Update,
            (
                camera::control,
                camera::simulation_reach,
                hud::read_actions,
                tools::input,
                town::upgrade_looks,
                build::refresh,
                draw::redraw_buildings,
                streets::redraw_streets,
                effects::tend_plumes,
                (
                    logistics::dispatch_carts,
                    logistics::drive_carts,
                    walkers::walk,
                )
                    .chain(),
                fleet::launch_smacks,
                fleet::plan_legs,
                fleet::sail,
                fleet::spin_props,
                draw::spin_parts,
                draw::follow_bodies,
                effects::follow_riders,
                tools::ghost,
                hud::update_data,
            )
                .chain(),
        );
    api::register(app);
}

/// Build the art, the islands, the start and the view.
fn setup(world: &mut World) {
    effects::register(world);
    let art = draw::make_art(world);
    let m = &art.mats;
    use art::Mat;
    world.insert_resource(build::StallKit(engine_ui::demo::art::stall::StallMats {
        wood: m.get(Mat::Planks),
        canvas: m.get(Mat::Canvas),
        iron: m.get(Mat::Iron),
        crystal: m.get(Mat::Window),
        pottery: m.get(Mat::Clay),
        burlap: m.get(Mat::Canvas),
        lamp_glass: m.get(Mat::Lamp),
        produce: m.get(Mat::Crop),
        cloth: m.get(Mat::Wool),
        wicker: m.get(Mat::Thatch),
        steel: m.get(Mat::Iron),
        brass: m.get(Mat::Copper),
    }));
    world.insert_resource(art);
    world::spawn_world(world);
    world::plant_woods(world);
    town::build_start(world);
    build::refresh(world);
    town::prewarm(world, 600.0);
    tools::setup(world);
    hud::setup(world);
    let isle = &island::islands()[0];
    let focus = world
        .get_resource::<town::TownCentre>()
        .map_or(DVec3::new(isle.center.x, 0.0, isle.center.y), |t| t.0);
    let mut cam = camera::StrategyCam::new(focus, 0.6, 260.0);
    cam.snap();
    let t = cam.transform();
    world.spawn((
        Name::new("Strategy camera"),
        Camera::perspective(50.0),
        t,
        cam,
    ));
}
