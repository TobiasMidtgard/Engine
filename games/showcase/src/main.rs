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

mod build;
mod camera;
mod film;
mod forest;
mod garden;
mod landscape;
mod loops;
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
mod walk;
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
        .add_systems(Startup, build::start)
        // The showcase's `&mut World` systems run only while they have work, so the rest of
        // the frame isn't held up behind them (AGENTS.md "Let systems run side by side").
        .init_resource::<build::Uploads>()
        .add_systems(
            Update,
            (
                build::watch_uploads,
                build::step.run_if(resource_exists::<build::WorldBuild>),
            )
                .chain(),
        )
        .add_systems(
            Update,
            (
                camera::fly,
                places::run_pending.run_if(resource_exists::<places::PendingVisit>),
                scenery::update_lods,
            )
                .chain(),
        )
        .add_systems(Update, forest::install)
        .add_systems(Update, paint_cache::track_eye.after(camera::fly))
        .add_systems(
            Update,
            (scenery::add_trunk_solids, scenery::stream_near_solids).chain(),
        )
        .add_systems(
            Update,
            (scenery::stream_wall_heroes, scenery::pilot_interacts),
        )
        .add_systems(Update, report_assets);
    register_apis(&mut app);

    let args: Vec<String> = std::env::args().collect();
    if !args.iter().any(|a| a == "--no-planet") {
        app.insert_resource(planet::UsePlanet);
        // The world re-anchors on the Moon or Mars when the camera gets there, and moves its
        // origin under it when it lands far from the valley, so every landing is as steady as
        // at home; jumps to places laid out in the valley take the world home first
        // (`camera::go_to`). North is +Z, as for the weather.
        let mut frame = app.world.resource_mut::<engine_space::SpaceFrame>();
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
    let headless = app.world.resource::<EngineConfig>().headless;
    let scripted = ["--tour", "--film", "--fly", "--loops"]
        .iter()
        .any(|f| args.iter().any(|a| a == f));
    let exe = !headless && !scripted;
    walk::install(&mut app, exe);
    if exe {
        // The window draws a loading screen at once while the world builds a step a frame,
        // and Movement's look bakes on its own thread meanwhile.
        app.insert_resource(build::Staged)
            .init_resource::<build::HeldJump>()
            .init_resource::<build::CoverOrder>()
            .add_systems(
                Update,
                (
                    build::track_jumps,
                    build::apply_cover.run_if(build::cover_ordered),
                )
                    .chain()
                    .after(camera::fly)
                    .after(build::watch_uploads),
            );
        app.insert_resource(engine_movement::look::LookSettings {
            background: true,
            ..Default::default()
        });
    }
    app.insert_resource(scenery::LodScale(
        value("--lod-scale").map_or(scenery::LodScale::default().0, |k| {
            k.parse().expect("--lod-scale <x>")
        }),
    ));
    if let Some(m) = value("--trees-within") {
        let m = if m == "all" {
            f64::INFINITY
        } else {
            m.parse().expect("--trees-within <m>|all")
        };
        app.insert_resource(scenery::TreeReach(m));
    }
    match value("--gardens") {
        Some("off") => {
            app.insert_resource(garden::NoGardens);
        }
        Some("on") | None => {}
        Some(other) => panic!("--gardens on|off, not {other}"),
    }
    if let Some(spec) = value("--loops") {
        loops::install(&mut app, spec);
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
