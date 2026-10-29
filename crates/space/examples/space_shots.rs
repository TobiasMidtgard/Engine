//! Headless pictures of the solar system's far views (Jupiter's bands, Saturn's rings, the
//! blue Earth, planets as dots in the sky), for reviews and the screenshot folder.
//!
//! ```text
//! cargo run --release -p engine_space --example space_shots -- [out_dir] [WxH] [names…]
//! cargo run --release -p engine_space --example space_shots -- --maps [out_dir] [width] [names…]
//! ```
//!
//! `--maps` bakes every body's far-view maps at full size, saves them as PNG (equirectangular,
//! Earth with its clouds on top) and prints how long each bake took.

use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, capture_now};
use engine_space::look::{FULL_MAP_WIDTH, LookBakes, bake_rings, bake_surface};
use engine_space::{SpaceClock, SpacePlugin, SpaceState, StarLight, demo, system, travel_to};
use engine_world::{SimulationFocus, WeatherState};

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
    /// Look out along the horizon, this many degrees below it, instead of at the body.
    horizon_deg: Option<f64>,
    /// The star that lights it.
    star: &'static str,
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
        horizon_deg: None,
        star: "Sun",
    };
    // Planets of the nearby stars (seeded: the names are what the generator makes).
    let exo = |name, body, star, phase_deg, ev100| View {
        star,
        ..v(name, body, 3.0, phase_deg, 15.0, ev100)
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
        // Low orbit and the edge of space over the far Earth, looking out to the horizon.
        View {
            fov: 60.0,
            horizon_deg: Some(14.0),
            ..v("earth_leo", "Earth", 1.0 + 400.0 / 6371.0, 20.0, 38.0, 14.6)
        },
        View {
            fov: 60.0,
            horizon_deg: Some(4.0),
            ..v("earth_edge", "Earth", 1.0 + 35.0 / 6371.0, 20.0, 38.0, 14.6)
        },
        // The same over Mars: its dusty air from low orbit and from 30 km up.
        View {
            fov: 60.0,
            horizon_deg: Some(12.0),
            ..v("mars_low", "Mars", 1.0 + 300.0 / 3389.5, 20.0, 38.0, 14.0)
        },
        View {
            fov: 60.0,
            horizon_deg: Some(4.0),
            ..v("mars_edge", "Mars", 1.0 + 30.0 / 3389.5, 20.0, 38.0, 14.0)
        },
        // Out past the planets: the Sun from Neptune, a bright point; a star close up; the
        // Alpha Centauri pair from between them.
        v("sun_from_neptune", "Neptune", 3.0, 0.0, 0.0, 9.0),
        v("sirius_close", "Sirius", 25.0, 0.0, 10.0, 34.5),
        View {
            fov: 60.0,
            ..v("alpha_centauri", "Alpha Centauri A", 2_000.0, 0.0, 0.0, 9.0)
        },
        exo(
            "proxima_b",
            "Proxima Centauri b",
            "Proxima Centauri",
            40.0,
            14.0,
        ),
        exo(
            "acen_b_frozen",
            "Alpha Centauri B f",
            "Alpha Centauri B",
            40.0,
            14.6,
        ),
        exo(
            "eps_eri_ocean",
            "Epsilon Eridani f",
            "Epsilon Eridani",
            40.0,
            14.6,
        ),
        exo(
            "acen_a_lava_night",
            "Alpha Centauri A c",
            "Alpha Centauri A",
            120.0,
            16.0,
        ),
        exo(
            "acen_a_lava_day",
            "Alpha Centauri A c",
            "Alpha Centauri A",
            30.0,
            23.0,
        ),
        exo(
            "proxima_ice",
            "Proxima Centauri e",
            "Proxima Centauri",
            40.0,
            13.6,
        ),
        exo(
            "barnard_airless",
            "Barnard's Star b",
            "Barnard's Star",
            40.0,
            17.5,
        ),
        exo(
            "acen_a_cold_desert",
            "Alpha Centauri A g",
            "Alpha Centauri A",
            40.0,
            14.0,
        ),
        exo("procyon_desert", "Procyon d", "Procyon", 40.0, 21.0),
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

/// CPU time this process has used so far (user + system), seconds; 0 where unknown. Bake
/// times on a busy machine are better judged by CPU time than by the clock.
fn cpu_seconds() -> f32 {
    let Ok(stat) = std::fs::read_to_string("/proc/self/stat") else {
        return 0.0;
    };
    // Fields after the parenthesized command name; utime and stime are the 12th and 13th.
    let rest = stat.rsplit_once(')').map(|(_, r)| r).unwrap_or("");
    let f: Vec<f32> = rest
        .split_whitespace()
        .skip(11)
        .take(2)
        .filter_map(|v| v.parse().ok())
        .collect();
    f.iter().sum::<f32>() / 100.0
}

/// Bake every body's maps at full size, save them and print the bake times.
fn maps(out: &std::path::Path, width: u32, only: &[String]) {
    std::fs::create_dir_all(out).expect("output folder");
    let started = std::time::Instant::now();
    let mut total = 0.0;
    let mut total_cpu = 0.0;
    for body in system::solar_system() {
        if body.look == engine_space::BodyLook::None
            || !(only.is_empty() || only.iter().any(|n| n.eq_ignore_ascii_case(&body.name)))
        {
            continue;
        }
        let cpu = cpu_seconds();
        let maps = bake_surface(&body, width);
        let cpu = cpu_seconds() - cpu;
        total += maps.seconds;
        total_cpu += cpu;
        let a = &maps.albedo;
        let x = &maps.aux;
        let mut rgba = a.data.clone();
        for (i, px) in rgba.chunks_mut(4).enumerate() {
            if maps.clouds {
                let c = x.data[i * 4] as f32 / 255.0;
                for v in &mut px[..3] {
                    *v = (*v as f32 * (1.0 - c) + 235.0 * c) as u8;
                }
            }
            px[3] = 255;
        }
        let name = body.name.to_lowercase();
        image::save_buffer(
            out.join(format!("{name}_map.png")),
            &rgba,
            a.width,
            a.height,
            image::ColorType::Rgba8,
        )
        .expect("save map");
        if maps.relief {
            let shade: Vec<u8> = x
                .data
                .chunks(4)
                .flat_map(|p| {
                    let v = (128.0 + (p[1] as f32 - 128.0) * 1.5 + (p[2] as f32 - 128.0) * 1.5)
                        .clamp(0.0, 255.0) as u8;
                    [v, v, v, 255]
                })
                .collect();
            image::save_buffer(
                out.join(format!("{name}_relief.png")),
                &shade,
                a.width,
                a.height,
                image::ColorType::Rgba8,
            )
            .expect("save relief");
        }
        if let Some(rings) = &body.rings {
            let r = bake_rings(rings, 4096);
            let strip: Vec<u8> = (0..64)
                .flat_map(|_| r.data.chunks(4).flat_map(|p| [p[0], p[1], p[2], p[3]]))
                .collect();
            image::save_buffer(
                out.join(format!("{name}_rings.png")),
                &strip,
                r.width,
                64,
                image::ColorType::Rgba8,
            )
            .expect("save rings");
        }
        println!(
            "{:<10} {:>4}x{:<4} {:>6.0} ms  ({:>5.0} ms cpu)",
            body.name,
            a.width,
            a.height,
            maps.seconds * 1000.0,
            cpu * 1000.0
        );
    }
    println!(
        "total      {:>6.0} ms of baking ({:.0} ms wall, {:.0} ms cpu over {} threads)",
        total * 1000.0,
        started.elapsed().as_secs_f32() * 1000.0,
        total_cpu * 1000.0,
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1)
    );
}

fn main() {
    engine_core::log::init("warn");
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--maps") {
        let out = args.get(1).cloned().unwrap_or_else(|| "space_maps".into());
        let width = args
            .get(2)
            .and_then(|w| w.parse().ok())
            .unwrap_or(FULL_MAP_WIDTH);
        maps(
            std::path::Path::new(&out),
            width,
            args.get(3..).unwrap_or(&[]),
        );
        return;
    }
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
    // `cover=0.7` gives the weather's clouds that cover, which the cloud tops near the camera
    // match; the rest name views.
    let cover: Option<f32> = args
        .iter()
        .skip(2)
        .find_map(|a| a.strip_prefix("cover=")?.parse().ok());
    let only: Vec<&str> = args
        .iter()
        .skip(2)
        .map(String::as_str)
        .filter(|a| !a.starts_with("cover="))
        .collect();
    std::fs::create_dir_all(&out).expect("output folder");

    for view in views() {
        if !only.is_empty() && !only.contains(&view.name) {
            continue;
        }
        let started = std::time::Instant::now();
        let mut app = build(size);
        if let Some(cloud_cover) = cover {
            app.world.insert_resource(WeatherState {
                cloud_cover,
                ..Default::default()
            });
        }
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
                s.body(view.star).unwrap().world_position,
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
        let (eye, target) = if view.name == "sun_from_neptune" {
            // Just sunward of Neptune, looking at the Sun 30 AU away.
            (center + to_sun * radius * view.distance_radii, sun)
        } else if view.name == "alpha_centauri" {
            // Off to the side of A and B, both in view.
            let b = app
                .world
                .resource::<SpaceState>()
                .body("Alpha Centauri B")
                .unwrap()
                .world_position;
            let mid = (center + b) * 0.5;
            let across = (b - center).cross(DVec3::Y).normalize();
            (mid + across * (b - center).length() * 1.2, mid)
        } else if view.name == "moon_and_earth" {
            // Behind the Moon and above it, looking at Earth over the Moon's limb.
            let to_earth = (earth - center).normalize();
            let up = to_earth.cross(side).normalize();
            (
                center - to_earth * radius * view.distance_radii + up * radius * 1.25,
                earth,
            )
        } else if let Some(down) = view.horizon_deg {
            // Along the horizon, across the sunlight, tipped down a little.
            let eye = center + dir * radius * view.distance_radii;
            let across = (side - dir * side.dot(dir)).normalize();
            let (sd, cd) = down.to_radians().sin_cos();
            (eye, eye + (across * cd - dir * sd) * 1.0e5)
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
            Transform::from_translation(eye).looking_at(
                target,
                if view.horizon_deg.is_some() {
                    (eye - center).normalize().as_vec3()
                } else {
                    pole.as_vec3()
                },
            ),
        ));
        app.update_n(4);
        // Full-detail maps bake in the background: wait for them.
        let wait = std::time::Instant::now();
        while app.world.resource::<LookBakes>().pending > 0 && wait.elapsed().as_secs() < 120 {
            std::thread::sleep(std::time::Duration::from_millis(20));
            app.update();
        }
        app.update_n(2);
        let path = out.join(format!("{}.png", view.name));
        let bakes = app.world.resource::<LookBakes>().clone();
        let baked: Vec<String> = bakes
            .done
            .iter()
            .filter(|(_, w, _)| *w >= FULL_MAP_WIDTH)
            .map(|(n, _, s)| format!("{n} {:.0} ms", s * 1000.0))
            .collect();
        match capture_now(&mut app.world, &path) {
            Ok(_) => println!(
                "{:<16} {:>6.1}s  -> {}  (baked: {})",
                view.name,
                started.elapsed().as_secs_f32(),
                path.display(),
                baked.join(", ")
            ),
            Err(e) => eprintln!("{}: {e}", view.name),
        }
    }
}
