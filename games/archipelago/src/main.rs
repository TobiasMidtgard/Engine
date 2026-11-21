//! Brightsund (working title Archipelago): a city-building and trade game on a chain of
//! islands, the engine's test of many buildings, carts, ships and aircraft at once (see
//! `engine-design/anno-like-game.md`). The title screen, menus and how-to-play are in `title.rs`.
//!
//! ```sh
//! cargo run -p archipelago --release                          # play a new game
//! cargo run -p archipelago --release -- --sandbox             # start with a finished town
//! cargo run -p archipelago --release -- --headless --tour     # writes screenshots/archipelago/*.png
//! cargo run -p archipelago --release -- --headless --tour town,harbour
//! cargo run -p archipelago --release -- --headless --play 240     # a scripted player, see playtest.rs
//! ```
//!
//! Keys: WASD or arrows pan (Shift faster), Q/E turn, wheel zooms, right-drag pans, middle-drag
//! turns. Pick a building in the bar at the bottom, R turns it, left click places it, right
//! click or Esc cancels. 1/2/3 set the game speed, Space pauses.

mod api;
mod art;
mod blueprint;
mod brain;
mod build;
mod burgher;
mod camera;
mod catalog;
mod characters;
mod cheats;
mod civic;
mod defs;
mod draw;
mod effects;
mod fields;
mod fittings;
mod flagships;
mod fleet;
mod fog;
mod gear;
mod globe;
mod grid;
mod halls;
mod helm;
mod hud;
mod icons;
mod incidents;
mod island;
mod items;
mod ledger;
mod logistics;
mod looks;
mod market;
mod orders;
mod patrician;
mod placing;
mod playtest;
mod regional;
mod reports;
mod rivals;
mod rules;
mod sea;
mod settle;
mod shipyard;
mod sim;
mod stamps;
mod streets;
mod title;
mod tools;
mod tour;
mod town;
mod tradelines;
mod traders;
mod unlocks;
mod vessels;
mod walkers;
mod world;

use engine::prelude::*;

fn main() {
    let mut app = App::new();
    engine::core::log::init("info,wgpu=warn,naga=warn");
    app.world.resource_mut::<EngineConfig>().app_name = title::NAME.into();
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
    title::add(&mut app);
    // `--sandbox`: start with the finished town of earlier rounds (the tour always does).
    if args.iter().any(|a| a == "--sandbox") || tour.is_some() {
        app.insert_resource(settle::StartMode::Sandbox);
    }
    // `--rivals N [--rival-level admiral]`: a new game with N rival houses (0 to 3).
    let level_arg = |flag: &str| {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
            .and_then(|s| brain::Level::from_name(s))
    };
    if let Some(n) = args
        .iter()
        .position(|a| a == "--rivals")
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.parse::<usize>().ok())
    {
        app.insert_resource(brain::RivalSetup {
            count: n.min(3),
            level: level_arg("--rival-level").unwrap_or(brain::Level::Captain),
        });
    }
    // `--rules economy=hard,military=ruthless,...`: the game's rules (see `rules.rs`).
    if let Some(text) = args
        .iter()
        .position(|a| a == "--rules")
        .and_then(|i| args.get(i + 1))
    {
        let mut r = rules::Rules::default();
        match r.parse(text) {
            Ok(()) => rules::apply(&mut app.world, r),
            Err(e) => engine::core::log::warn!("archipelago: --rules: {e}"),
        }
    }
    // `--rival-picks gullwick:admiral,vane,hask:mate`: the characters played against.
    if let Some(text) = args
        .iter()
        .position(|a| a == "--rival-picks")
        .and_then(|i| args.get(i + 1))
    {
        let level = level_arg("--rival-level").unwrap_or(brain::Level::Captain);
        match characters::RivalPicks::parse(text, level) {
            Ok(p) => {
                // (Picks alone start a game with that many rivals.)
                if app.world.get_resource::<brain::RivalSetup>().is_none() {
                    app.insert_resource(brain::RivalSetup {
                        count: p.0.len(),
                        level,
                    });
                }
                app.insert_resource(p);
            }
            Err(e) => engine::core::log::warn!("archipelago: --rival-picks: {e}"),
        }
    }
    // `--play-level admiral`: how well the scripted player plays (Captain unless told).
    if let Some(l) = level_arg("--play-level") {
        app.insert_resource(brain::Seat { knobs: l.knobs() });
    }
    // `--play <game minutes> [--play-speed 4] [--play-shots town,harbour]`: a scripted player.
    if let Some(i) = args.iter().position(|a| a == "--play") {
        let minutes = args
            .get(i + 1)
            .and_then(|s| s.parse().ok())
            .unwrap_or(120.0);
        let speed = args
            .iter()
            .position(|a| a == "--play-speed")
            .and_then(|i| args.get(i + 1))
            .and_then(|s| s.parse().ok())
            .unwrap_or(4.0);
        let shots = args
            .iter()
            .position(|a| a == "--play-shots")
            .and_then(|i| args.get(i + 1))
            .cloned();
        // `--seed N` varies the incidents and the player's habits; `--play-out DIR` keeps runs
        // apart (the economy rig runs several seeds).
        let seed = args
            .iter()
            .position(|a| a == "--seed")
            .and_then(|i| args.get(i + 1))
            .and_then(|s| s.parse().ok())
            .unwrap_or(1);
        let out = args
            .iter()
            .position(|a| a == "--play-out")
            .and_then(|i| args.get(i + 1))
            .cloned()
            .unwrap_or_else(|| "screenshots/archipelago/playtest".to_string());
        playtest::run(app, minutes, speed, shots.as_deref(), seed, &out);
        return;
    }
    if args.iter().any(|a| a == "--title-shots") {
        title::shots(app);
        return;
    }
    // `--helm-shots [keys]`: pictures of picking and ordering ships (helm.rs).
    if let Some(i) = args.iter().position(|a| a == "--helm-shots") {
        let only = args.get(i + 1).filter(|s| !s.starts_with("--")).cloned();
        helm::shots(app, only.as_deref());
        return;
    }
    // `--flagship-shots [keys]`: pictures of the flagships at work (helm.rs).
    if let Some(i) = args.iter().position(|a| a == "--flagship-shots") {
        let only = args.get(i + 1).filter(|s| !s.starts_with("--")).cloned();
        helm::flagship_shots(app, only.as_deref());
        return;
    }
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
        .init_resource::<items::Items>()
        .init_resource::<world::Woods>()
        .init_resource::<settle::StartMode>()
        .init_resource::<settle::Settled>()
        .init_resource::<shipyard::Launched>()
        .init_resource::<incidents::Incidents>()
        .init_resource::<fittings::Designs>()
        .init_resource::<brain::Captains>()
        .add_systems(Startup, setup)
        .add_systems(FixedUpdate, sim::economy_system)
        .add_systems(
            Update,
            (
                (
                    camera::control,
                    camera::simulation_reach,
                    world::follow_air,
                    world::tend_woods,
                )
                    .chain(),
                hud::read_actions,
                tools::input,
                town::upgrade_looks,
                (
                    items::tick,
                    incidents::tick,
                    traders::tick,
                    characters::diplomacy,
                    reports::alerts,
                )
                    .chain(),
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
                (fleet::launch_smacks, shipyard::tick, brain::tick).chain(),
                fleet::plan_legs,
                fleet::sail,
                fleet::spin_props,
                draw::spin_parts,
                draw::follow_bodies,
                effects::follow_riders,
                (
                    fog::update,
                    tradelines::update,
                    helm::update,
                    flagships::update,
                )
                    .chain(),
                tools::ghost,
                hud::update_data,
            )
                .chain(),
        );
    globe::add(app);
    api::register(app);
    reports::register(app);
    rivals::register(app);
    tradelines::register(app);
    traders::register(app);
    rules::register(app);
    characters::register(app);
    cheats::register(app);
    settle::register(app);
    helm::register(app);
    incidents::register(app);
    fittings::register(app);
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
    sea::spawn(world);
    match *world.resource::<settle::StartMode>() {
        settle::StartMode::Sandbox => {
            town::build_start(world);
            if let Some(r) = world.get_resource::<brain::RivalSetup>().copied()
                && r.count > 0
            {
                brain::adopt_far_towns(world, r.count, r.level);
            }
        }
        settle::StartMode::Fresh => {
            settle::fresh(world);
            if let Some(r) = world.get_resource::<brain::RivalSetup>().copied()
                && r.count > 0
            {
                brain::start_rivals(world, r.count, r.level);
            }
        }
    }
    fog::start(world);
    build::refresh(world);
    town::prewarm(world, 600.0);
    tools::setup(world);
    hud::setup(world);
    helm::setup(world);
    let isle = &island::islands()[0];
    let focus = world
        .get_resource::<town::TownCentre>()
        .map_or(DVec3::new(isle.center.x, 0.0, isle.center.y), |t| t.0);
    let mut cam = camera::StrategyCam::new(focus, 0.6, 260.0);
    cam.snap();
    let t = cam.transform(world.resource::<globe::Frame>());
    world.spawn((
        Name::new("Strategy camera"),
        Camera::perspective(50.0),
        t,
        cam,
    ));
}
