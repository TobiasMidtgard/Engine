//! Headless pictures of the solar system's far views (Jupiter's bands, Saturn's rings, the
//! blue Earth, planets as dots in the sky), for reviews and the screenshot folder.
//!
//! ```text
//! cargo run --release -p engine_space --example space_shots -- [out_dir] [WxH] [names…]
//! ```

use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, capture_now};
use engine_space::{SpaceClock, SpacePlugin, SpaceState, StarLight, demo, travel_to};
use engine_world::SimulationFocus;

struct View {
    name: &'static str,
    body: &'static str,
    /// Camera distance from the body's center, in body radii.
    distance_radii: f64,
    /// Angle between the sun and the camera seen from the body (0 = sun behind the camera).
    phase_deg: f64,
    /// Camera height above the body's orbit plane, degrees.
    elevation_deg: f64,
    ev100: f32,
    fov: f32,
}

fn views() -> Vec<View> {
    let v = |name, body, distance_radii, phase_deg, elevation_deg, ev100| View {
        name,
        body,
        distance_radii,
        phase_deg,
        elevation_deg,
        ev100,
        fov: 40.0,
    };
    vec![
        v("jupiter", "Jupiter", 4.0, 35.0, 8.0, 11.0),
        v("saturn", "Saturn", 7.0, 30.0, 22.0, 9.6),
        v("earth_far", "Earth", 4.5, 50.0, 15.0, 15.3),
        v("venus", "Venus", 4.0, 60.0, 10.0, 16.2),
        v("mars_far", "Mars", 4.0, 40.0, 10.0, 14.2),
        v("uranus", "Uranus", 4.0, 30.0, 10.0, 7.6),
        v("neptune", "Neptune", 4.0, 30.0, 10.0, 6.4),
        View {
            fov: 60.0,
            ..v("moon_and_earth", "Moon", 3.5, 120.0, 5.0, 15.3)
        },
    ]
}

fn build(size: (u32, u32)) -> App {
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
        .add_plugins(SpacePlugin);
    let mut commands = app.world.commands();
    demo::spawn_solar_system(&mut commands);
    app.world.flush();
    app.world.resource_mut::<SpaceClock>().paused = true;
    app.world.spawn((
        Transform::default(),
        SimulationFocus::default(),
        engine_world::physics::Velocity::default(),
    ));
    app.world.spawn((
        DirectionalLight::default(),
        StarLight::default(),
        Transform::default(),
    ));
    app
}

fn main() {
    engine_core::log::init("warn");
    let args: Vec<String> = std::env::args().skip(1).collect();
    let out = std::path::PathBuf::from(
        args.first()
            .cloned()
            .unwrap_or_else(|| "space_shots".into()),
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
        let mut app = build(size);
        app.update();
        if let Some(d) = app.world.get_resource::<RenderDisabled>() {
            eprintln!("no GPU: {}", d.0);
            return;
        }
        // Anchor the world on the body, then put the camera where the view wants it.
        let radius = {
            let s = app.world.resource::<SpaceState>();
            s.body(view.body).expect("body").radius_m
        };
        travel_to(
            &mut app.world,
            view.body,
            0.0,
            0.0,
            radius * (view.distance_radii - 1.0),
        )
        .expect("travel");
        app.update_n(2);
        let (center, sun, earth, pole) = {
            let s = app.world.resource::<SpaceState>();
            let b = s.body(view.body).unwrap();
            (
                b.world_position,
                s.body("Sun").unwrap().world_position,
                s.body("Earth").unwrap().world_position,
                b.world_rotation * DVec3::Y,
            )
        };
        // Frame the shot around the body's pole, as photos of planets are.
        let to_sun = (sun - center).normalize();
        let side = to_sun.cross(pole).normalize();
        let flat_sun = side.cross(pole).normalize() * -1.0;
        let (sp, cp) = view.phase_deg.to_radians().sin_cos();
        let (se, ce) = view.elevation_deg.to_radians().sin_cos();
        let dir = ((flat_sun * cp + side * sp) * ce + pole * se).normalize();
        let (eye, target) = if view.name == "moon_and_earth" {
            // Behind the Moon and above it, looking at Earth over the Moon's limb.
            let to_earth = (earth - center).normalize();
            let up = to_earth.cross(side).normalize();
            (
                center - to_earth * radius * view.distance_radii + up * radius * 1.25,
                earth,
            )
        } else {
            (center + dir * radius * view.distance_radii, center)
        };
        {
            let mut q = app
                .world
                .query_filtered::<&mut Transform, With<SimulationFocus>>();
            q.single_mut(&mut app.world).unwrap().translation = eye;
        }
        app.world.spawn((
            Camera {
                exposure: Exposure::Manual { ev100: view.ev100 },
                ..Camera::perspective(view.fov)
            },
            Transform::from_translation(eye).looking_at(target, pole.as_vec3()),
        ));
        app.update_n(4);
        let path = out.join(format!("{}.png", view.name));
        match capture_now(&mut app.world, &path) {
            Ok(_) => println!(
                "{:<16} {:>6.1}s  -> {}",
                view.name,
                started.elapsed().as_secs_f32(),
                path.display()
            ),
            Err(e) => eprintln!("{}: {e}", view.name),
        }
    }
}
