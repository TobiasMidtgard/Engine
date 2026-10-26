//! The engine's tech showcase: one map with a zone per system, a camera that flies from the
//! ground to the Moon, a day–night cycle and an automated screenshot tour.
//!
//! ```sh
//! cargo run -p showcase --release                       # fly around (see camera.rs for keys)
//! cargo run -p showcase --release -- --headless --tour  # writes screenshots/tour/*.png
//! cargo run -p showcase --release -- --headless --tour plaza,orbit
//! cargo run -p showcase --release -- --headless --tour paddock_car,yard   # loads 2 zones
//! cargo run -p showcase --release -- --headless --film overview,plaza,falls --secs 6
//! cargo run -p showcase --release -- --headless --film footsteps --follow "Trail hiker" --secs 60
//! ```
//!
//! `--film` saves frames and sound for a video (see `film.rs`); `--follow` films an entity, found
//! by its `Name`, from behind. The valley lies on a real planet (`planet.rs`); `--no-planet`
//! draws it as the landscape's own mesh on an empty ground instead. A tour or film whose stops
//! all belong to zones spawns only those zones' demos; `--zones a,b` (or `none`, `all`) picks
//! them (see `zones::demo_zones`).
//!
//! The map is a valley ringed by snowy mountains: a village around the material plaza, woods,
//! a lake, and one zone per engine feature. Each zone shows placeholder content until its module
//! ships a `demo::spawn_demo` (see `zones.rs`); `TEST_CASES.md` lists every test case and its
//! owner.

mod camera;
mod film;
mod forest;
mod landscape;
mod meshgen;
mod paint_cache;
mod placeholders;
mod places;
mod planet;
mod rock_textures;
mod rocks;
mod scenery;
mod texgen;
mod tour;
mod tower;
mod village;
mod walls;
mod zones;

use engine::prelude::*;

fn main() {
    let mut app = App::new();
    engine::core::log::init("info,wgpu=warn,naga=warn");
    app.world.resource_mut::<EngineConfig>().app_name = "Engine showcase".into();
    engine::apply_cli_args(&mut app);

    app.add_plugins(DefaultPlugins)
        .add_plugins((
            engine_physics::PhysicsPlugin,
            engine_vehicles::VehiclesPlugin,
            engine_space::SpacePlugin,
            engine_terrain::TerrainPlugin,
            engine_water::WaterPlugin,
            engine_weather::WeatherPlugin,
            engine_fire::FirePlugin,
            engine_smoke::SmokePlugin,
        ))
        .add_plugins((
            engine_vfx::VfxPlugin,
            engine_foliage::FoliagePlugin,
            engine_anim::AnimationPlugin,
            engine_npc::NpcPlugin,
            engine_ui::UiPlugin,
            engine_destruction::DestructionPlugin,
            engine_movement::MovementPlugin,
            engine_audio::AudioPlugin,
        ))
        .add_plugins(engine_softbody::SoftBodyPlugin)
        // People with an Appearance are drawn by the character creator's module (faces that
        // blink, look around and wince); anim still poses them.
        .add_plugins(engine_character::CharacterPlugin)
        .add_systems(Startup, setup)
        .add_systems(
            Update,
            (camera::fly, places::run_pending, scenery::update_lods).chain(),
        )
        .add_systems(Update, forest::install)
        .add_systems(Update, paint_cache::track_eye.after(camera::fly))
        .add_systems(Update, scenery::stream_wall_colliders)
        .add_systems(Update, scenery::stream_wall_heroes)
        .add_systems(Update, report_assets);
    register_apis(&mut app);

    let args: Vec<String> = std::env::args().collect();
    if !args.iter().any(|a| a == "--no-planet") {
        app.insert_resource(planet::UsePlanet);
        // Stay anchored on Earth, turning with it, even at the Moon: tour stops jump between
        // the ground and space in valley coordinates. North is +Z, as for the weather.
        let mut frame = app.world.resource_mut::<engine_space::SpaceFrame>();
        frame.auto_switch = false;
        frame.home_heading_deg = planet::HEADING_DEG;
        // The weather runs the day; the Sun, the Moon and the planets keep to its clock.
        app.world
            .resource_mut::<engine_space::SpaceSettings>()
            .follow_world_clock = true;
    }
    let value = |flag: &str| {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
            .filter(|a| !a.starts_with("--"))
            .map(String::as_str)
    };
    let film_keys = value("--film").unwrap_or("overview");
    let stops = if args.iter().any(|a| a == "--tour") {
        value("--tour")
    } else if args.iter().any(|a| a == "--film") {
        Some(film_keys)
    } else {
        None
    };
    if let Some(zones) = zones::demo_zones(value("--zones"), stops) {
        app.insert_resource(zones);
    }
    if args.iter().any(|a| a == "--tour") {
        tour::install(&mut app, value("--tour"));
    } else if args.iter().any(|a| a == "--film") {
        let num = |flag: &str, default: f32| {
            args.iter()
                .position(|a| a == flag)
                .and_then(|i| args.get(i + 1))
                .and_then(|v| v.parse().ok())
                .unwrap_or(default)
        };
        let follow = args
            .iter()
            .position(|a| a == "--follow")
            .and_then(|i| args.get(i + 1))
            .cloned();
        film::install(
            &mut app,
            film_keys,
            num("--secs", 8.0),
            num("--fps", 30.0),
            follow,
        );
    }
    app.run();
}

fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    (mut atmosphere, mut settings, mut fog, mut surfaces): (
        ResMut<Atmosphere>,
        ResMut<RenderSettings>,
        ResMut<FogSettings>,
        ResMut<engine::world::SurfaceMaterials>,
    ),
    config: Res<EngineConfig>,
    (use_planet, demo_zones, plants): (
        Option<Res<planet::UsePlanet>>,
        Option<Res<zones::DemoZones>>,
        Res<engine_foliage::PlantLibrary>,
    ),
) {
    let t0 = std::time::Instant::now();
    // Each step's share of the build, for `--profile` runs chasing a slow start.
    let mut laps: Vec<(&str, f32)> = Vec::new();
    let mut last = t0;
    let mut lap = |what: &'static str| {
        let now = std::time::Instant::now();
        laps.push((what, (now - last).as_secs_f32()));
        last = now;
    };
    // The planet beyond the terrain is grassland too, so the far edge blends into it.
    atmosphere.ground_color = [0.07, 0.13, 0.04];
    // Mountains a few km away should still be in the shadow maps' reach for the valley.
    settings.shadow_distance = settings.shadow_distance.max(400.0);
    // Every image in the showcase and its module demos is built once and only sampled on the
    // GPU (audited for round 11), so the CPU copies can go once they are uploaded.
    settings.release_image_pixels = true;
    // Clear mountain air: a faint valley haze, about 130 km visibility (3.9 / density), so the
    // atmosphere gives the distant ranges their blue and the far shadow map their shade.
    fog.density = 0.00003;

    let land = landscape::shared().clone();
    let ground = std::sync::Arc::new(landscape::LandscapeGround::new(land.clone(), &mut surfaces));
    // On the planet the terrain answers `Ground` (edits included) and asks this for surfaces.
    commands.insert_resource(engine::world::Ground(ground.clone()));
    let lib = scenery::build_library(&mut meshes, &mut materials, &mut images, &plants);
    lap("library");
    let mut rng = Rng::new(config.seed);
    if use_planet.is_some() {
        planet::spawn(&mut commands, land.clone(), ground);
    } else {
        scenery::spawn_terrain(&mut commands, &mut meshes, &lib, &land);
    }
    lap("terrain");
    scenery::spawn_woods(&mut commands, &lib, &land, &mut rng);
    lap("woods");
    scenery::spawn_meadows(&mut commands, &lib, &land, &mut rng);
    scenery::spawn_meadow_grass(&mut commands, &land);
    lap("meadows");
    let hedged = scenery::spawn_farms(&mut commands, &lib, &land, &mut rng);
    scenery::spawn_walls(&mut commands, &lib, &land, &hedged);
    lap("farms and walls");
    scenery::spawn_rocks(&mut commands, &lib, &land, &mut rng);
    lap("rocks");
    scenery::spawn_village(&mut commands, &lib, &land, &mut rng);
    lap("village");
    scenery::spawn_cliffs_and_stream(&mut commands, &mut meshes, &lib, &land, &mut rng);
    lap("cliffs and stream");
    placeholders::cave_shell(&mut commands, &mut meshes, &lib, &land, &mut rng);
    lap("cave");
    if let Some(only) = &demo_zones {
        engine::core::log::info!(
            "only the zone demos of {:?} (`--zones all` spawns every one)",
            only.0
        );
    }
    zones::spawn_demos(&mut commands, &lib, &land, &mut rng, demo_zones.as_deref());
    lap("zone demos");
    if use_planet.is_none() {
        zones::moon(&mut commands);
    }
    camera::spawn_camera(&mut commands);
    // The weather module drives the sun, the moon and the clock (`engine_weather::Sky`).
    engine_weather::spawn_sky_lights(&mut commands);
    // The tower's textures baked on other threads while the world spawned.
    lib.tower.finish_textures(&mut images);
    lap("tower textures");
    let laps: Vec<String> = laps.iter().map(|(w, s)| format!("{w} {s:.2}")).collect();
    engine::core::log::info!(
        "showcase world built in {:.1} s ({})",
        t0.elapsed().as_secs_f32(),
        laps.join(", ")
    );
}

/// Log how much memory the meshes and images hold, by owner (the first part of each asset's
/// key: `vehicles/cars/…` is the vehicles', `trail:planks` the trail's) and the biggest, at
/// frames 2 and 10 (a tour stop's first and last) and every 600. The renderer keeps a GPU copy
/// of each as well, in system memory under a software renderer.
fn report_assets(meshes: Res<Assets<Mesh>>, images: Res<Assets<Image>>, mut frames: Local<u32>) {
    *frames += 1;
    if ![2, 10].contains(&*frames) && !frames.is_multiple_of(600) {
        return;
    }
    let vertex = std::mem::size_of::<engine::render::Vertex>();
    log_assets(&meshes, "meshes", |m| {
        m.vertices.len() * vertex + m.indices.len() * 4
    });
    log_assets(&images, "images", |i| {
        i.data.len() + i.mips.iter().map(Vec::len).sum::<usize>()
    });
}

fn log_assets<T: Asset>(assets: &Assets<T>, what: &str, bytes: impl Fn(&T) -> usize) {
    const MB: f64 = 1024.0 * 1024.0;
    let mb = |b: usize| format!("{:.0} MB", b as f64 / MB);
    let keys: std::collections::HashMap<u32, &str> =
        assets.keys().map(|(k, h)| (h.index(), k)).collect();
    let mut sizes: Vec<(usize, u32)> = assets.iter().map(|(h, a)| (bytes(a), h.index())).collect();
    sizes.sort_unstable_by(|a, b| b.cmp(a));
    let mut owners: std::collections::HashMap<&str, usize> = Default::default();
    for (b, i) in &sizes {
        let owner = keys
            .get(i)
            .map_or("unnamed", |k| k.split(['/', ':']).next().unwrap_or(k));
        *owners.entry(owner).or_default() += b;
    }
    let mut owners: Vec<(&str, usize)> = owners.into_iter().filter(|o| o.1 as f64 >= MB).collect();
    owners.sort_unstable_by_key(|o| std::cmp::Reverse(o.1));
    let owners: Vec<String> = owners
        .iter()
        .map(|(o, b)| format!("{o} {}", mb(*b)))
        .collect();
    let biggest: Vec<String> = sizes
        .iter()
        .take(12)
        .map(|(b, i)| match keys.get(i) {
            Some(k) => format!("{k} {}", mb(*b)),
            None => format!("#{i} {}", mb(*b)),
        })
        .collect();
    engine::core::log::info!(
        "memory: {} {what}, {}; by owner: {}; biggest: {}",
        sizes.len(),
        mb(sizes.iter().map(|s| s.0).sum()),
        owners.join(", "),
        biggest.join(", ")
    );
}

fn register_apis(app: &mut App) {
    app.register_api(
        "showcase.zones",
        "List the map's zones: where each is and which module's demo belongs there.",
        "{}",
        |_, _| {
            Ok(serde_json::json!(
                zones::ZONES
                    .iter()
                    .map(|z| serde_json::json!({
                        "key": z.key, "name": z.name, "center": z.center.to_array(),
                        "radius": z.radius, "module": z.module,
                    }))
                    .collect::<Vec<_>>()
            ))
        },
    )
    .register_api(
        "showcase.places",
        "List the camera viewpoints (zones, orbit, Moon).",
        "{}",
        |_, _| {
            Ok(serde_json::json!(
                places::places()
                    .iter()
                    .map(|p| serde_json::json!({ "key": p.key, "name": p.name, "hour": p.hour }))
                    .collect::<Vec<_>>()
            ))
        },
    )
    .register_api(
        "showcase.goto",
        "Move the camera to a viewpoint. Set \"with_time\" to also use its time of day.",
        r#"{"place": "lake", "with_time": false}"#,
        |world, args| {
            let key = args
                .get("place")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            let Some(place) = places::find(key) else {
                let keys: Vec<_> = places::places().iter().map(|p| p.key).collect();
                return Err(format!(
                    "Unknown place \"{key}\". Use one of: {}.",
                    keys.join(", ")
                ));
            };
            camera::go_to(world, &place);
            places::visit_soon(world, &place);
            if args
                .get("with_time")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
            {
                world.resource_mut::<engine_weather::Sky>().hour = place.hour;
            }
            Ok(serde_json::json!({ "place": place.key, "position": place.position.to_array() }))
        },
    )
    .register_api(
        "showcase.time",
        "Set the hour of day (0–24) and optionally how many game hours pass per second.",
        r#"{"hour": 18.0, "hours_per_second": 0.0}"#,
        |world, args| {
            let mut sky = world.resource_mut::<engine_weather::Sky>();
            if let Some(h) = args.get("hour").and_then(|v| v.as_f64()) {
                sky.hour = (h as f32).rem_euclid(24.0);
            }
            if let Some(s) = args.get("hours_per_second").and_then(|v| v.as_f64()) {
                sky.hours_per_second = s as f32;
            }
            Ok(serde_json::json!({ "hour": sky.hour, "hours_per_second": sky.hours_per_second }))
        },
    );
}
