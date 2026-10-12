//! Renders the movement demo's set dressing (the parkour course and the duel ring) headless,
//! from fixed cameras and at a few times of day, to PNGs.
//!
//! ```text
//! cargo run -p engine_movement --release --example course_preview -- [out_dir] [shot ...]
//! ```
//!
//! Shots: `overview`, `slide_bar`, `walls`, `tower_top`, `ring_golden`, `ring_dusk`,
//! `level_column` (all by default). `name=ex,ey,ez,tx,ty,tz[,fov,sun_elevation,sun_azimuth]`
//! adds a custom shot (camera eye and target in meters, angles in degrees). The showcase
//! surrounds the demo with terrain; here a ground plane and the sky stand in for it. Needs a
//! GPU or a software Vulkan driver (lavapipe).

use std::path::PathBuf;

use engine_core::math::{DVec3, Vec3};
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_movement::MovementPlugin;
use engine_movement::demo::{self, DuelDirector, Round};
use engine_render::prelude::*;
use engine_render::{RenderDisabled, capture_now};
use engine_world::WorldPlugin;

const WIDTH: u32 = 1600;
const HEIGHT: u32 = 900;
const DT: f64 = 1.0 / 60.0;

/// One picture: where the camera is, where the sun is, and how it is exposed.
struct Shot {
    name: &'static str,
    eye: [f64; 3],
    target: [f64; 3],
    fov_deg: f32,
    /// Sun elevation and azimuth (degrees; azimuth 0 = from +X, 90 = from −Z).
    sun_elevation_deg: f32,
    sun_azimuth_deg: f32,
    /// Exposure compensation, EV.
    compensation: f32,
    /// Height fog density at the ground, per meter.
    fog: f32,
}

const SHOTS: &[Shot] = &[
    Shot {
        name: "overview",
        eye: [-28.0, 15.0, -36.0],
        target: [3.0, 0.0, 0.0],
        fov_deg: 40.0,
        sun_elevation_deg: 22.0,
        sun_azimuth_deg: 65.0,
        compensation: 0.0,
        fog: 0.002,
    },
    Shot {
        name: "slide_bar",
        eye: [-19.5, 1.5, 7.4],
        target: [-14.0, 1.2, 12.5],
        fov_deg: 50.0,
        sun_elevation_deg: 28.0,
        sun_azimuth_deg: 35.0,
        compensation: 0.0,
        fog: 0.002,
    },
    Shot {
        name: "walls",
        eye: [-9.5, 2.2, 10.5],
        target: [4.0, 2.4, 12.4],
        fov_deg: 60.0,
        sun_elevation_deg: 30.0,
        sun_azimuth_deg: 60.0,
        compensation: 0.0,
        fog: 0.002,
    },
    Shot {
        name: "tower_top",
        eye: [16.0, 8.4, 5.5],
        target: [22.5, 5.6, 11.5],
        fov_deg: 55.0,
        sun_elevation_deg: 24.0,
        sun_azimuth_deg: 20.0,
        compensation: 0.0,
        fog: 0.002,
    },
    Shot {
        name: "ring_golden",
        eye: [-11.0, 3.4, 1.0],
        target: [0.5, 1.0, -15.0],
        fov_deg: 50.0,
        sun_elevation_deg: 7.0,
        sun_azimuth_deg: 200.0,
        compensation: 0.0,
        fog: 0.0025,
    },
    Shot {
        name: "ring_dusk",
        eye: [12.0, 3.2, -1.5],
        target: [-1.0, 1.0, -15.5],
        fov_deg: 52.0,
        sun_elevation_deg: -9.0,
        sun_azimuth_deg: 160.0,
        compensation: 0.0,
        fog: 0.0022,
    },
    Shot {
        name: "level_column",
        eye: [16.8, 1.7, -12.6],
        target: [9.5, 2.9, -14.0],
        fov_deg: 52.0,
        sun_elevation_deg: -3.0,
        sun_azimuth_deg: 160.0,
        compensation: 0.0,
        fog: 0.002,
    },
];

fn sun_dir(elevation_deg: f32, azimuth_deg: f32) -> Vec3 {
    let (e, a) = (elevation_deg.to_radians(), azimuth_deg.to_radians());
    Vec3::new(a.cos() * e.cos(), e.sin(), -a.sin() * e.cos()).normalize()
}

fn main() {
    let mut args = std::env::args().skip(1);
    let out = args
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target/course_preview"));
    let only: Vec<String> = args.collect();
    std::fs::create_dir_all(&out).expect("output directory");

    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = WIDTH;
        w.height = HEIGHT;
    }
    app.add_plugins(RenderPlugin)
        .add_plugins(WorldPlugin)
        .add_plugins(MovementPlugin);
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual { step_secs: DT });
    app.world.resource_mut::<FixedTime>().set_hz(60.0);
    app.world
        .insert_resource(RenderDisabled("not drawing yet".into()));
    {
        let mut s = app.world.resource_mut::<RenderSettings>();
        s.bloom_intensity = 0.05;
        s.vignette = 0.22;
        s.shadow_distance = 120.0;
    }

    let sun = app
        .world
        .spawn((
            DirectionalLight {
                color: [1.0, 0.97, 0.92],
                illuminance: 110_000.0,
                ..Default::default()
            },
            Transform::default(),
        ))
        .id();
    // Ground: the showcase puts terrain here.
    app.world.spawn((
        Transform::from_xyz(0.0, -0.002, 0.0),
        MeshRenderer::new(
            Shape::Plane {
                size: [600.0, 600.0],
                subdivisions: 1,
            },
            StandardMaterial::hex(0x5a554b).with_roughness(1.0),
        ),
    ));
    let camera = app
        .world
        .spawn((
            Camera::perspective(50.0).with_exposure(Exposure::Auto {
                min_ev100: 2.0,
                max_ev100: 16.0,
                speed: 60.0,
                compensation: 0.0,
            }),
            Transform::from_xyz(0.0, 5.0, 20.0),
        ))
        .id();

    let mut queue = bevy_ecs::world::CommandQueue::default();
    let mut commands = Commands::new(&mut queue, &app.world);
    demo::spawn_demo(&mut commands, DVec3::ZERO);
    queue.apply(&mut app.world);
    {
        let mut q = app.world.query::<&mut DuelDirector>();
        let mut d = q.single_mut(&mut app.world).unwrap();
        d.round = Round::Focus;
        d.only = Some(Round::Focus);
    }
    // Let the characters settle (and the look library build) without drawing.
    for _ in 0..20 {
        app.update();
    }
    app.world.remove_resource::<RenderDisabled>();

    let custom: Vec<Shot> = only
        .iter()
        .filter_map(|o| {
            let (name, nums) = o.split_once('=')?;
            let v: Vec<f64> = nums.split(',').filter_map(|n| n.parse().ok()).collect();
            (v.len() >= 6).then(|| Shot {
                name: Box::leak(name.to_string().into_boxed_str()),
                eye: [v[0], v[1], v[2]],
                target: [v[3], v[4], v[5]],
                fov_deg: v.get(6).copied().unwrap_or(50.0) as f32,
                sun_elevation_deg: v.get(7).copied().unwrap_or(30.0) as f32,
                sun_azimuth_deg: v.get(8).copied().unwrap_or(40.0) as f32,
                compensation: 0.0,
                fog: 0.002,
            })
        })
        .collect();
    let wanted = |name: &str| only.is_empty() || only.iter().any(|o| o == name);
    for shot in SHOTS.iter().filter(|s| wanted(s.name)).chain(&custom) {
        let dir = sun_dir(shot.sun_elevation_deg, shot.sun_azimuth_deg);
        *app.world.get_mut::<Transform>(sun).unwrap() =
            Transform::default().looking_at(-dir.as_dvec3(), Vec3::Y);
        app.world.resource_mut::<FogSettings>().density = shot.fog;
        *app.world.get_mut::<Transform>(camera).unwrap() =
            Transform::from_translation(DVec3::from_array(shot.eye))
                .looking_at(DVec3::from_array(shot.target), Vec3::Y);
        {
            let mut c = app.world.get_mut::<Camera>(camera).unwrap();
            c.projection = Projection::Perspective {
                fov_y_deg: shot.fov_deg,
                near: 0.05,
            };
            c.exposure = Exposure::Auto {
                min_ev100: 2.0,
                max_ev100: 16.0,
                speed: 60.0,
                compensation: shot.compensation,
            };
        }
        // A few frames for auto exposure (and the shadow cascades) to settle on the view.
        for _ in 0..8 {
            app.update();
            if let Some(d) = app.world.get_resource::<RenderDisabled>() {
                eprintln!("cannot render: {}", d.0);
                std::process::exit(1);
            }
        }
        let path = out.join(format!("{}.png", shot.name));
        match capture_now(&mut app.world, &path) {
            Ok(_) => println!("saved {}", path.display()),
            Err(e) => eprintln!("{}: {e}", shot.name),
        }
    }
}
