//! Headless pictures of planets and terrain, for reviews and the project's screenshot folder.
//!
//! ```text
//! cargo run --release -p engine_terrain --example terrain_shots -- [out_dir] [WxH] [names…]
//! ```
//!
//! Writes one PNG per view (all views when no names are given). Needs a GPU or lavapipe.

use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, capture_now};
use engine_terrain::{FlatPad, PlanetTerrain, TerrainPlugin, demo};
use engine_world::{PrecipitationKind, SimulationFocus, SurfaceLayers, WeatherState};

#[derive(Clone, Copy)]
enum World3 {
    Earth,
    Moon,
    Mars,
}

#[derive(Clone, Copy, Default)]
enum Ground {
    #[default]
    Dry,
    Rain,
    Snow,
}

struct View {
    name: &'static str,
    world: World3,
    eye: DVec3,
    target: DVec3,
    /// Place `eye` this high above the ground below it instead of using `eye.y`.
    above_ground: Option<f64>,
    /// Sun azimuth (degrees from north toward east) and elevation (degrees).
    sun: (f64, f64),
    ev100: f32,
    fov: f32,
    lab: bool,
    ground: Ground,
}

const BASE: View = View {
    name: "",
    world: World3::Earth,
    eye: DVec3::ZERO,
    target: DVec3::ZERO,
    above_ground: None,
    sun: (220.0, 35.0),
    ev100: 14.5,
    fov: 60.0,
    lab: false,
    ground: Ground::Dry,
};

fn views() -> Vec<View> {
    vec![
        View {
            name: "lab_prints",
            eye: DVec3::new(-21.0, 1.9, 9.0),
            target: DVec3::new(-13.0, 0.0, 1.0),
            sun: (250.0, 18.0),
            lab: true,
            ..BASE
        },
        View {
            name: "lab_tracks",
            eye: DVec3::new(2.0, 2.4, 13.0),
            target: DVec3::new(8.0, 0.0, 5.0),
            sun: (250.0, 20.0),
            lab: true,
            ..BASE
        },
        View {
            name: "lab_overview",
            eye: DVec3::new(-4.0, 22.0, 34.0),
            target: DVec3::new(0.0, 0.0, -6.0),
            sun: (230.0, 32.0),
            lab: true,
            ..BASE
        },
        View {
            name: "lab_rain",
            eye: DVec3::new(-21.0, 1.9, 9.0),
            target: DVec3::new(-13.0, 0.0, 1.0),
            sun: (250.0, 18.0),
            ev100: 13.5,
            lab: true,
            ground: Ground::Rain,
            ..BASE
        },
        View {
            name: "lab_snow",
            eye: DVec3::new(-4.0, 22.0, 34.0),
            target: DVec3::new(0.0, 0.0, -6.0),
            sun: (230.0, 25.0),
            ev100: 15.0,
            lab: true,
            ground: Ground::Snow,
            ..BASE
        },
        View {
            name: "earth_range",
            eye: DVec3::new(-600.0, 0.0, -1_500.0),
            target: DVec3::new(2_000.0, 1_200.0, -32_000.0),
            above_ground: Some(25.0),
            sun: (245.0, 16.0),
            ..BASE
        },
        View {
            name: "earth_coast",
            eye: DVec3::new(-7_000.0, 0.0, 12_500.0),
            target: DVec3::new(-17_000.0, -400.0, 20_000.0),
            above_ground: Some(250.0),
            sun: (255.0, 12.0),
            ev100: 14.8,
            ..BASE
        },
        View {
            name: "earth_mountains",
            eye: DVec3::new(-14_000.0, 0.0, -19_000.0),
            target: DVec3::new(6_000.0, 1_800.0, -33_000.0),
            above_ground: Some(1_200.0),
            sun: (250.0, 22.0),
            ..BASE
        },
        View {
            name: "earth_high",
            eye: DVec3::new(-10_000.0, 45_000.0, 90_000.0),
            target: DVec3::new(0.0, 0.0, -30_000.0),
            sun: (230.0, 28.0),
            fov: 65.0,
            ..BASE
        },
        View {
            name: "earth_orbit",
            eye: DVec3::new(0.0, 2_600_000.0, 7_000_000.0),
            target: DVec3::new(0.0, -6_371_000.0, 0.0),
            sun: (120.0, 40.0),
            fov: 50.0,
            ..BASE
        },
        View {
            name: "moon_surface",
            world: World3::Moon,
            eye: DVec3::new(3_500.0, 0.0, -2_500.0),
            target: DVec3::new(9_000.0, -200.0, -9_000.0),
            above_ground: Some(1.8),
            sun: (200.0, 12.0),
            ev100: 15.0,
            ..BASE
        },
        View {
            name: "moon_low_orbit",
            world: World3::Moon,
            eye: DVec3::new(0.0, 60_000.0, 120_000.0),
            target: DVec3::new(0.0, -40_000.0, -150_000.0),
            sun: (260.0, 15.0),
            ev100: 15.0,
            fov: 70.0,
            ..BASE
        },
        View {
            name: "moon_orbit",
            world: World3::Moon,
            eye: DVec3::new(0.0, 900_000.0, 3_200_000.0),
            target: DVec3::new(0.0, -1_737_400.0, 0.0),
            sun: (100.0, 20.0),
            ev100: 15.0,
            fov: 50.0,
            ..BASE
        },
        View {
            name: "mars_orbit",
            world: World3::Mars,
            eye: DVec3::new(0.0, 1_500_000.0, 6_000_000.0),
            target: DVec3::new(0.0, -3_389_500.0, 0.0),
            sun: (110.0, 30.0),
            ev100: 14.5,
            fov: 50.0,
            ..BASE
        },
    ]
}

fn sun_dir(azimuth_deg: f64, elevation_deg: f64) -> DVec3 {
    let (az, el) = (azimuth_deg.to_radians(), elevation_deg.to_radians());
    // North is −Z, east is +X.
    DVec3::new(el.cos() * az.sin(), el.sin(), -el.cos() * az.cos())
}

fn build(view: &View, size: (u32, u32)) -> App {
    let mut app = App::new();
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual { step_secs: 0.25 });
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = size.0;
        w.height = size.1;
    }
    app.add_plugins(engine_world::WorldPlugin)
        .add_plugins(RenderPlugin)
        .add_plugins(TerrainPlugin);

    let pad = FlatPad::default();
    let planet = match view.world {
        World3::Earth => PlanetTerrain::earthlike(7),
        World3::Moon => PlanetTerrain::moon(3),
        World3::Mars => PlanetTerrain::mars(5),
    }
    .with_pad(pad);
    let tf = planet.home_transform(pad);
    {
        let mut atmo = app.world.resource_mut::<Atmosphere>();
        match view.world {
            World3::Earth => {
                atmo.planet_center = tf.translation;
                atmo.planet_radius = planet.shape.radius_m;
            }
            World3::Moon | World3::Mars => atmo.enabled = false,
        }
    }
    app.world.spawn((tf, planet));
    app.world
        .spawn((Transform::default(), SimulationFocus::default()));
    let sun = sun_dir(view.sun.0, view.sun.1);
    app.world.spawn((
        DirectionalLight::default(),
        Transform::default().looking_at(-sun, Vec3::Y),
    ));
    if view.lab {
        let mut commands = app.world.commands();
        demo::spawn_demo(&mut commands, DVec3::ZERO);
        app.world.flush();
    }
    if !view.lab {
        // Clear air for landscapes, so the pictures show the terrain rather than the haze.
        let mut w = app.world.resource_mut::<WeatherState>();
        w.fog = 0.0;
        w.cloud_cover = 0.15;
        // Clear-day visibility (~100 km) instead of the default ground haze.
        app.world.resource_mut::<FogSettings>().density = 0.00002;
    }
    match view.ground {
        Ground::Dry => {}
        Ground::Rain => {
            let mut w = app.world.resource_mut::<WeatherState>();
            w.precipitation_mm_h = 6.0;
            w.precipitation_kind = PrecipitationKind::Rain;
            w.cloud_cover = 0.6;
            let mut layers = app.world.resource_mut::<SurfaceLayers>();
            let (width, height) = (layers.grid.width, layers.grid.height);
            for y in 0..height {
                for x in 0..width {
                    let i = (y * width + x) as usize;
                    let c = &mut layers.grid.data[i];
                    c.wetness = 0.95;
                    // A few shallow puddles.
                    let (fx, fy) = (x as f32 * 0.37, y as f32 * 0.29);
                    let n = (fx.sin() * fy.cos() + (fx * 0.5 + 1.3).cos() * (fy * 0.7).sin()) * 0.5;
                    c.puddle_m = (n - 0.55).max(0.0) * 0.08;
                }
            }
        }
        Ground::Snow => {
            let mut w = app.world.resource_mut::<WeatherState>();
            w.temperature_c = -6.0;
            for c in &mut app.world.resource_mut::<SurfaceLayers>().grid.data {
                c.snow_m = 0.18;
            }
        }
    }
    app
}

fn main() {
    engine_core::log::init("warn");
    let args: Vec<String> = std::env::args().skip(1).collect();
    let out = std::path::PathBuf::from(
        args.first()
            .cloned()
            .unwrap_or_else(|| "terrain_shots".into()),
    );
    let size = args
        .get(1)
        .and_then(|s| s.split_once('x'))
        .and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)))
        .unwrap_or((1280, 720));
    let only: Vec<&str> = args.iter().skip(2).map(String::as_str).collect();
    std::fs::create_dir_all(&out).expect("output folder");

    for view in views() {
        if !only.is_empty() && !only.contains(&view.name) {
            continue;
        }
        let started = std::time::Instant::now();
        let mut app = build(&view, size);
        app.update();
        if let Some(d) = app.world.get_resource::<RenderDisabled>() {
            eprintln!("no GPU: {}", d.0);
            return;
        }
        let mut eye = view.eye;
        if let Some(h) = view.above_ground {
            let ground = app.world.resource::<engine_world::Ground>().clone();
            let y = ground
                .height(DVec3::new(eye.x, 50_000.0, eye.z))
                .unwrap_or(0.0);
            eye.y = y + h;
        }
        app.world.spawn((
            Camera {
                exposure: Exposure::Manual { ev100: view.ev100 },
                ..Camera::perspective(view.fov)
            },
            Transform::from_translation(eye).looking_at(view.target, Vec3::Y),
        ));
        // Let the walkers lay their trails and the terrain finish streaming, with the ground
        // materials baked.
        let settings = app
            .world
            .get_resource::<engine_terrain::TerrainSettings>()
            .cloned()
            .unwrap_or_default();
        let (resolution, scans) = (
            settings.material_resolution,
            settings.scan_folder(&app.world),
        );
        while resolution > 0
            && engine_terrain::materials::shared(resolution, scans.as_deref()).is_none()
        {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        app.update_n(12);
        let path = out.join(format!("{}.png", view.name));
        match capture_now(&mut app.world, &path) {
            Ok(_) => {
                let d = app
                    .world
                    .resource::<engine_core::diagnostics::Diagnostics>();
                println!(
                    "{:<16} {:>6.1}s  patches drawn {:>5}  resident {:>5}  -> {}",
                    view.name,
                    started.elapsed().as_secs_f32(),
                    d.get("terrain.patches_drawn").unwrap_or(0.0),
                    d.get("terrain.patches_resident").unwrap_or(0.0),
                    path.display()
                );
            }
            Err(e) => eprintln!("{}: {e}", view.name),
        }
    }
}
