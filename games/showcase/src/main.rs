//! The engine's tech showcase: one map with a zone per system, a camera that flies from the
//! ground to the Moon, a day–night cycle and an automated screenshot tour.
//!
//! ```sh
//! cargo run -p showcase --release                       # fly around (see camera.rs for keys)
//! cargo run -p showcase --release -- --headless --tour  # writes screenshots/tour/*.png
//! cargo run -p showcase --release -- --headless --tour plaza,orbit
//! ```
//!
//! The map is a valley ringed by snowy mountains: a village around the material plaza, woods,
//! a lake, and one zone per engine feature. Each zone shows placeholder content until its module
//! ships a `demo::spawn_demo` (see `zones.rs`); `TEST_CASES.md` lists every test case and its
//! owner.

mod camera;
mod landscape;
mod meshgen;
mod placeholders;
mod places;
mod scenery;
mod tour;
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
        .add_systems(Startup, setup)
        .add_systems(Update, (camera::fly, scenery::update_lods).chain());
    register_apis(&mut app);

    let args: Vec<String> = std::env::args().collect();
    if let Some(i) = args.iter().position(|a| a == "--tour") {
        let only = args
            .get(i + 1)
            .filter(|a| !a.starts_with("--"))
            .map(String::as_str);
        tour::install(&mut app, only);
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
) {
    let t0 = std::time::Instant::now();
    // The planet beyond the terrain is grassland too, so the far edge blends into it.
    atmosphere.ground_color = [0.07, 0.13, 0.04];
    // Mountains a few km away should still be in the shadow maps' reach for the valley.
    settings.shadow_distance = settings.shadow_distance.max(400.0);
    // Clear mountain air: a light valley haze (the weather module drives this later).
    fog.density = 0.00012;

    let land = landscape::shared().clone();
    commands.insert_resource(engine::world::Ground(std::sync::Arc::new(
        landscape::LandscapeGround::new(land.clone(), &mut surfaces),
    )));
    let lib = scenery::build_library(&mut meshes, &mut materials, &mut images);
    let mut rng = Rng::new(config.seed);
    scenery::spawn_terrain(&mut commands, &mut meshes, &lib, &land);
    scenery::spawn_woods(&mut commands, &lib, &land, &mut rng);
    scenery::spawn_meadows(&mut commands, &lib, &land, &mut rng);
    scenery::spawn_farms(&mut commands, &lib, &land, &mut rng);
    scenery::spawn_rocks(&mut commands, &lib, &land, &mut rng);
    scenery::spawn_village(&mut commands, &lib, &land, &mut rng);
    scenery::spawn_cliffs_and_stream(&mut commands, &lib, &land, &mut rng);
    placeholders::cave_shell(&mut commands, &mut meshes, &lib, &land, &mut rng);
    zones::spawn_demos(&mut commands, &mut meshes, &lib, &land, &mut rng);
    zones::moon(&mut commands);
    camera::spawn_camera(&mut commands);
    // The weather module drives the sun, the moon and the clock (`engine_weather::Sky`).
    engine_weather::spawn_sky_lights(&mut commands);
    engine::core::log::info!(
        "showcase world built in {:.1} s",
        t0.elapsed().as_secs_f32()
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
            places::visit(world, &place);
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
