//! Headless pictures of planets as a ship closes in on them, with the space module's far view
//! and the terrain module's ground drawn together the way the showcase draws them: to check
//! that the handover from one to the other can't be seen.
//!
//! ```text
//! cargo run --release -p approach_shots -- [out_dir] [WxH] [names…]
//! cargo run --release -p approach_shots -- [out_dir] [WxH] flight [stops…]
//! ```
//!
//! Each view is shot twice, `<name>_far.png` with the far view alone and `<name>_ground.png`
//! with the planet's terrain too, from the same place. Both are set up as the showcase sets
//! them up (games/showcase/src/planet.rs): an Earth-like world's far view painted from its
//! terrain's ground, the terrain wearing the far view's look from high up, the cloud tops kept
//! over the ground. With `MAPS=1` the far look the ground wears is written as a flat map too.
//!
//! `flight` shoots one trip instead, stop by stop, with the showcase's adapting exposure: from
//! a field on Earth to orbit, out past the Sun to Proxima Centauri and down onto Proxima b's
//! ground (`<stop>.png` and all of them on `flight.jpg`).

use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, capture_now};
use engine_space::look::{FarGround, GroundDrawn, GroundPoint, LookBakes};
use engine_space::nearby::{SurfaceRecipe, WorldClass};
use engine_space::{
    CelestialBody, SpaceClock, SpacePlugin, SpaceState, StarLight, demo, travel_to,
};
use engine_terrain::far_look::FAR_LOOK_WIDTH;
use engine_terrain::{FarLook, FarLookMap, PlanetTerrain, TerrainPlugin, WorldKind, WorldRecipe};
use engine_world::SimulationFocus;
use std::sync::Arc;

struct View {
    name: &'static str,
    body: &'static str,
    /// The star that lights it.
    star: &'static str,
    /// Camera distance from the body's center, in body radii.
    distance_radii: f64,
    /// Angle between the star and the camera seen from the body (0 = star behind the camera).
    phase_deg: f64,
    ev100: f32,
    fov: f32,
}

fn views() -> Vec<View> {
    let v = |name, body, star, distance_radii, phase_deg, ev100| View {
        name,
        body,
        star,
        distance_radii,
        phase_deg,
        ev100,
        fov: 40.0,
    };
    vec![
        v("earth_4r", "Earth", "Sun", 4.5, 50.0, 15.3),
        v(
            "proxima_b_3r",
            "Proxima Centauri b",
            "Proxima Centauri",
            3.0,
            40.0,
            14.0,
        ),
        v(
            "proxima_b_39r",
            "Proxima Centauri b",
            "Proxima Centauri",
            39.0,
            40.0,
            14.0,
        ),
        v(
            "eps_eri_ocean_3r",
            "Epsilon Eridani f",
            "Epsilon Eridani",
            3.0,
            40.0,
            14.6,
        ),
        v(
            "eps_eri_ocean_39r",
            "Epsilon Eridani f",
            "Epsilon Eridani",
            39.0,
            40.0,
            14.6,
        ),
        v(
            "lava_3r",
            "Alpha Centauri A c",
            "Alpha Centauri A",
            3.0,
            30.0,
            23.0,
        ),
        v(
            "lava_39r",
            "Alpha Centauri A c",
            "Alpha Centauri A",
            39.0,
            30.0,
            23.0,
        ),
        v(
            "ice_3r",
            "Proxima Centauri e",
            "Proxima Centauri",
            3.0,
            40.0,
            13.6,
        ),
        v(
            "ice_39r",
            "Proxima Centauri e",
            "Proxima Centauri",
            39.0,
            40.0,
            13.6,
        ),
        v("desert_3r", "Procyon d", "Procyon", 3.0, 40.0, 21.0),
        v("desert_39r", "Procyon d", "Procyon", 39.0, 40.0, 21.0),
        v(
            "airless_3r",
            "Barnard's Star b",
            "Barnard's Star",
            3.0,
            40.0,
            17.5,
        ),
        v(
            "cold_desert_3r",
            "Alpha Centauri A g",
            "Alpha Centauri A",
            3.0,
            40.0,
            14.0,
        ),
        // Locked worlds from the side: the warm eye under the star, the frozen night side.
        v(
            "proxima_b_side_3r",
            "Proxima Centauri b",
            "Proxima Centauri",
            3.0,
            85.0,
            14.0,
        ),
        v(
            "lava_side_3r",
            "Alpha Centauri A c",
            "Alpha Centauri A",
            3.0,
            85.0,
            23.0,
        ),
        // Coming down: a few hundred and a hundred kilometers up, where the ground's own
        // detail takes over from the far look.
        View {
            fov: 60.0,
            ..v(
                "lava_low",
                "Alpha Centauri A c",
                "Alpha Centauri A",
                1.05,
                30.0,
                23.0,
            )
        },
        View {
            fov: 60.0,
            ..v(
                "ice_low",
                "Proxima Centauri e",
                "Proxima Centauri",
                1.02,
                40.0,
                13.6,
            )
        },
        View {
            fov: 60.0,
            ..v("desert_low", "Procyon d", "Procyon", 1.02, 40.0, 21.0)
        },
        View {
            fov: 60.0,
            ..v(
                "proxima_b_low",
                "Proxima Centauri b",
                "Proxima Centauri",
                1.05,
                40.0,
                14.0,
            )
        },
    ]
}

/// The terrain recipe of a planet of another star (as the showcase makes it).
fn world_recipe(body: &CelestialBody, s: &SurfaceRecipe) -> WorldRecipe {
    WorldRecipe {
        kind: match s.class {
            WorldClass::Airless => WorldKind::Airless,
            WorldClass::Lava => WorldKind::Lava,
            WorldClass::Desert => WorldKind::Desert,
            WorldClass::Terra => WorldKind::Terra,
            WorldClass::Ocean => WorldKind::Ocean,
            WorldClass::ColdDesert => WorldKind::ColdDesert,
            WorldClass::Ice => WorldKind::Ice,
            WorldClass::Hazy => WorldKind::Hazy,
        },
        seed: s.seed,
        radius_m: body.radius_m,
        sea_fraction: s.sea_fraction,
        crater_density: s.crater_density,
        pressure_bar: s.pressure_bar,
        temperature_k: s.temperature_k,
        star_kelvin: s.star_kelvin,
        color: body.color,
        lava_glow_nits: engine_space::look::melt_nits(body),
        locked: body.tidally_locked,
    }
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
        .add_plugins(SpacePlugin)
        .add_plugins(TerrainPlugin);
    let mut commands = app.world.commands();
    demo::spawn_solar_system(&mut commands);
    app.world.flush();
    app.world.resource_mut::<SpaceClock>().paused = true;
    // No weather here to draw the clouds around the camera: the cloud layer covers it all.
    app.world
        .resource_mut::<engine_space::SpaceSettings>()
        .cloud_hole_m = 0.0;
    app.world.spawn((
        Transform::default(),
        SimulationFocus::default(),
        engine_world::physics::Velocity::default(),
    ));
    app.world.spawn((
        DirectionalLight {
            cast_shadows: std::env::var_os("NO_SHADOWS").is_none(),
            ..Default::default()
        },
        StarLight::default(),
        Transform::default(),
    ));
    app
}

/// Where a flight stop's camera is.
enum Eye {
    /// `distance_radii` from the body's center, `phase_deg` from the star (as [`View`]).
    Radii { distance_radii: f64, phase_deg: f64 },
    /// `altitude_m` above the ground where the star stands `sun_deg` high, looking along the
    /// ground `turn_deg` from the star's side toward it (0 = the star to the side, 90 = facing
    /// it), `down_deg` below the horizon (negative: above it).
    Ground {
        altitude_m: f64,
        sun_deg: f64,
        turn_deg: f64,
        down_deg: f64,
    },
    /// On the line from body `from` to body `to`, `from_m` out from `from`, looking on at
    /// `to` (or back at `from`).
    Between {
        from: &'static str,
        to: &'static str,
        from_m: f64,
        look_back: bool,
    },
}

/// One stop of a flight from the ground of one world to the ground of another's.
struct Stop {
    name: &'static str,
    body: &'static str,
    star: &'static str,
    eye: Eye,
    fov: f32,
}

const AU: f64 = 1.495_978_707e11;

/// From a field on Earth up to orbit, out of the solar system, across to Proxima Centauri and
/// down onto Proxima b, as one flight would see it.
fn flight() -> Vec<Stop> {
    let ground = |name, body, star, altitude_m: f64, sun_deg, down_deg| Stop {
        name,
        body,
        star,
        eye: Eye::Ground {
            altitude_m,
            sun_deg,
            turn_deg: 0.0,
            down_deg,
        },
        fov: 70.0,
    };
    let radii = |name, body, star, distance_radii, phase_deg| Stop {
        name,
        body,
        star,
        eye: Eye::Radii {
            distance_radii,
            phase_deg,
        },
        fov: 40.0,
    };
    let between = |name, body, from, to, from_m, look_back| Stop {
        name,
        body,
        star: body,
        eye: Eye::Between {
            from,
            to,
            from_m,
            look_back,
        },
        fov: 50.0,
    };
    const LIGHT_YEAR: f64 = 9.460_730_472_580_8e15;
    let proxima = "Proxima Centauri";
    let b = "Proxima Centauri b";
    vec![
        ground("01_earth_ground", "Earth", "Sun", 2.0, 30.0, 4.0),
        ground("02_earth_1km", "Earth", "Sun", 1_000.0, 30.0, 8.0),
        ground("03_earth_10km", "Earth", "Sun", 10_000.0, 30.0, 10.0),
        ground("04_earth_100km", "Earth", "Sun", 100_000.0, 30.0, 12.0),
        ground("05_earth_1000km", "Earth", "Sun", 1_000_000.0, 30.0, 15.0),
        radii("06_earth_4r", "Earth", "Sun", 4.5, 50.0),
        radii("07_earth_40r", "Earth", "Sun", 40.0, 50.0),
        between("08_sun_from_100au", "Sun", "Sun", proxima, 100.0 * AU, true),
        between(
            "09_proxima_from_3ly",
            proxima,
            "Sun",
            proxima,
            LIGHT_YEAR,
            false,
        ),
        between(
            "10_proxima_from_0.3au",
            proxima,
            proxima,
            "Sun",
            0.3 * AU,
            true,
        ),
        radii("11_proxima_b_39r", b, proxima, 39.0, 40.0),
        radii("12_proxima_b_3r", b, proxima, 3.0, 40.0),
        Stop {
            fov: 60.0,
            ..radii("13_proxima_b_1.05r", b, proxima, 1.05, 40.0)
        },
        // Inside the warm eye under the star, facing it.
        Stop {
            eye: Eye::Ground {
                altitude_m: 10_000.0,
                sun_deg: 40.0,
                turn_deg: 60.0,
                down_deg: 8.0,
            },
            ..ground("14_proxima_b_10km", b, proxima, 0.0, 0.0, 0.0)
        },
        Stop {
            eye: Eye::Ground {
                altitude_m: 2.0,
                sun_deg: 58.0,
                turn_deg: 80.0,
                down_deg: -22.0,
            },
            fov: 80.0,
            ..ground("15_proxima_b_ground", b, proxima, 0.0, 0.0, 0.0)
        },
    ]
}

/// The far view's sampler of a terrain's ground (as the showcase makes it).
fn far_ground(terrain: &PlanetTerrain) -> FarGround {
    let shape = Arc::new(terrain.shape.clone());
    let frost = terrain.palette.snow_below_c;
    FarGround::new(move |d, texel_m| {
        let s = shape.sample(d.as_dvec3(), (2.0 * texel_m as f64).max(1_000.0));
        GroundPoint {
            height_m: s.height as f32,
            moisture: s.moisture,
            above_frost_c: s.temperature_c - frost,
            temperature_c: s.temperature_c,
        }
    })
}

/// A planet's look from afar for its terrain (as the showcase makes it).
fn far_look(body: &CelestialBody, ground: Option<&FarGround>) -> FarLook {
    let maps = engine_space::look::bake_surface_on(body, ground, FAR_LOOK_WIDTH);
    let rgba = maps
        .albedo
        .data
        .as_chunks::<4>()
        .0
        .iter()
        .zip(maps.aux.data.as_chunks::<4>().0)
        .flat_map(|(a, x)| [a[0], a[1], a[2], x[3]])
        .collect();
    FarLook {
        map: Arc::new(FarLookMap::new(maps.albedo.width, maps.albedo.height, rgba)),
        near_tint: if body.look == engine_space::BodyLook::Earthlike {
            0.2
        } else {
            0.8
        },
    }
}

/// The terrain of `body`, if it has one.
fn terrain_of(b: &CelestialBody) -> Option<PlanetTerrain> {
    match &b.surface {
        Some(s) => Some(PlanetTerrain::world(&world_recipe(b, s))),
        None if b.name == "Earth" => Some(PlanetTerrain::earthlike(1).with_radius(b.radius_m)),
        None => None,
    }
}

/// Set `body` up the way the showcase does: an Earth-like planet of another star painted
/// from its ground from afar, and with `ground`, its terrain wearing the far look.
fn add_ground(app: &mut App, body: &str, ground: bool) {
    let mut q = app.world.query::<(Entity, &CelestialBody)>();
    let Some((entity, b)) = q
        .iter(&app.world)
        .find(|(_, b)| b.name == body)
        .map(|(e, b)| (e, b.clone()))
    else {
        return;
    };
    let Some(terrain) = terrain_of(&b) else {
        return;
    };
    // (`NO_AIR=1`: the planet without its air, to tell the air's part in a picture.)
    if std::env::var_os("NO_AIR").is_some()
        && let Some(mut body) = app.world.get_mut::<CelestialBody>(entity)
    {
        body.atmosphere = None;
        body.haze = None;
    }
    let sampler = (b.surface.is_some() && b.look == engine_space::BodyLook::Earthlike)
        .then(|| far_ground(&terrain));
    if let Some(s) = &sampler {
        app.world.entity_mut(entity).insert(s.clone());
    }
    if ground {
        let mut e = app.world.entity_mut(entity);
        e.insert((terrain, GroundDrawn));
        // (`NO_FAR_LOOK=1`: the ground alone, as before the far look.)
        if b.surface.is_some() && std::env::var_os("NO_FAR_LOOK").is_none() {
            e.insert(far_look(&b, sampler.as_ref()));
        }
    }
}

/// Write the far look of `view`'s body as an equirectangular map (north up, longitude −180°
/// on the left, the point under a locked world's star in the middle), glow in red on top.
fn save_map(app: &mut App, view: &View, out: &std::path::Path) {
    let mut q = app.world.query::<(&CelestialBody, &FarLook)>();
    let Some((_, look)) = q.iter(&app.world).find(|(b, _)| b.name == view.body) else {
        return;
    };
    let (w, h) = (FAR_LOOK_WIDTH, FAR_LOOK_WIDTH / 2);
    let rgb: Vec<u8> = look.map.levels()[0]
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|t| [t[0].max(t[3]), t[1], t[2]])
        .collect();
    let path = out.join(format!("{}_map.png", view.name));
    if let Err(e) = image::save_buffer(&path, &rgb, w, h, image::ExtendedColorType::Rgb8) {
        eprintln!("{}: {e}", path.display());
    }
}

/// Let the terrain stream in its patches (and the far view its full maps).
fn settle(app: &mut App) {
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
    app.update_n(8);
    let mut last_boost = 0.0;
    for _ in 0..300 {
        let d = app
            .world
            .resource::<engine_core::diagnostics::Diagnostics>();
        let built = d.get("terrain.patches_built").unwrap_or(0.0);
        let boost = d.get("terrain.detail_boost").unwrap_or(1.0);
        let pending = d.get("terrain.patches_building").unwrap_or(0.0)
            + d.get("terrain.patches_missing").unwrap_or(0.0);
        let baking = app.world.resource::<LookBakes>().pending;
        if built == 0.0 && pending == 0.0 && baking == 0 && (boost - last_boost).abs() < 0.005 {
            break;
        }
        last_boost = boost;
        if d.get("terrain.patches_building").unwrap_or(0.0) > 0.0 || baking > 0 {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        app.update();
    }
    app.update_n(2);
}

fn shoot(view: &View, ground: bool, size: (u32, u32), out: &std::path::Path) {
    let started = std::time::Instant::now();
    let mut app = build(size);
    app.update();
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("no GPU: {}", d.0);
        std::process::exit(1);
    }
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
    add_ground(&mut app, view.body, ground);
    // (`MAPS=1`: also the far look the ground wears, as a flat map: `<name>_map.png`.)
    if ground && std::env::var_os("MAPS").is_some() {
        save_map(&mut app, view, out);
    }
    app.update_n(2);
    let (center, star, pole) = {
        let s = app.world.resource::<SpaceState>();
        let b = s.body(view.body).unwrap();
        (
            b.world_position,
            s.body(view.star).unwrap().world_position,
            b.world_rotation * DVec3::Y,
        )
    };
    // Frame the shot around the body's pole, a little above its equator.
    let to_star = (star - center).normalize();
    let side = to_star.cross(pole).normalize();
    let flat_star = side.cross(pole).normalize() * -1.0;
    let (sp, cp) = view.phase_deg.to_radians().sin_cos();
    let (se, ce) = 15f64.to_radians().sin_cos();
    let dir = ((flat_star * cp + side * sp) * ce + pole * se).normalize();
    let eye = center + dir * radius * view.distance_radii;
    {
        let mut q = app
            .world
            .query_filtered::<&mut Transform, With<SimulationFocus>>();
        q.single_mut(&mut app.world).unwrap().translation = eye;
    }
    // Far out the planet is small: narrow the view so it fills about the same share.
    let fov = if view.distance_radii > 10.0 {
        view.fov * 3.0 / view.distance_radii as f32
    } else {
        view.fov
    };
    app.world.spawn((
        Camera {
            exposure: Exposure::Manual { ev100: view.ev100 },
            ..Camera::perspective(fov)
        },
        Transform::from_translation(eye).looking_at(center, pole.as_vec3()),
    ));
    settle(&mut app);
    // Aim again: the world may have shifted its origin under the camera meanwhile.
    let center = app
        .world
        .resource::<SpaceState>()
        .body(view.body)
        .unwrap()
        .world_position;
    let eye = {
        let mut q = app
            .world
            .query_filtered::<&Transform, With<SimulationFocus>>();
        q.single(&app.world).unwrap().translation
    };
    {
        let mut q = app.world.query_filtered::<&mut Transform, With<Camera>>();
        for mut tf in q.iter_mut(&mut app.world) {
            *tf = Transform::from_translation(eye).looking_at(center, pole.as_vec3());
        }
    }
    app.update_n(2);
    let tag = if ground { "ground" } else { "far" };
    let path = out.join(format!("{}_{tag}.png", view.name));
    match capture_now(&mut app.world, &path) {
        Ok(_) => println!(
            "{:<24} {:>6.1}s  -> {}",
            format!("{}_{tag}", view.name),
            started.elapsed().as_secs_f32(),
            path.display()
        ),
        Err(e) => eprintln!("{}: {e}", view.name),
    }
}

/// Where a stop's camera stands and what it looks at (world space, now).
fn place(app: &mut App, stop: &Stop) -> (DVec3, DVec3, DVec3) {
    let s = app.world.resource::<SpaceState>();
    let b = s.body(stop.body).expect("body");
    let (center, radius, rot) = (b.world_position, b.radius_m, b.world_rotation);
    let star = s.body(stop.star).expect("star").world_position;
    match stop.eye {
        Eye::Radii {
            distance_radii,
            phase_deg,
        } => {
            let pole = rot * DVec3::Y;
            let to_star = (star - center).normalize();
            let side = to_star.cross(pole).normalize();
            let flat_star = side.cross(pole).normalize() * -1.0;
            let (sp, cp) = phase_deg.to_radians().sin_cos();
            let (se, ce) = 15f64.to_radians().sin_cos();
            let dir = ((flat_star * cp + side * sp) * ce + pole * se).normalize();
            (center + dir * radius * distance_radii, center, pole)
        }
        Eye::Between {
            from,
            to,
            from_m,
            look_back,
        } => {
            let a = s.body(from).expect("from").world_position;
            let z = s.body(to).expect("to").world_position;
            let along = (z - a).normalize();
            let eye = a + along * from_m;
            // (Any up that isn't along the line of sight.)
            let up = if along.y.abs() > 0.9 {
                DVec3::Z
            } else {
                DVec3::Y
            };
            (eye, if look_back { a } else { z }, up)
        }
        Eye::Ground {
            altitude_m,
            sun_deg,
            turn_deg,
            down_deg,
        } => {
            let terrain = app
                .world
                .get::<PlanetTerrain>(b.entity)
                .map(|t| t.shape.clone());
            // Planet-local: the star's direction, and a spot `90° − sun_deg` from under it,
            // on land if there is some all round.
            let local_star = (rot.inverse() * (star - center)).normalize();
            let across = local_star.any_orthonormal_vector();
            let from_star = (90.0 - sun_deg).to_radians();
            // Height and moisture there.
            let ground = |d: DVec3| {
                terrain.as_ref().map_or((0.0, 0.0), |t| {
                    let s = t.sample(d, 1_000.0);
                    (s.height, s.moisture as f64)
                })
            };
            let spot = (0..72)
                .map(|k| {
                    let around = DQuat::from_axis_angle(local_star, k as f64 * 5f64.to_radians());
                    let d = DQuat::from_axis_angle(around * across, from_star) * local_star;
                    let (h, wet) = ground(d);
                    (d, h, wet)
                })
                .max_by(|x, y| {
                    // Land between 50 m and 1.5 km up beats everything else; then the wetter
                    // (where plants grow), then the higher.
                    let good = |h: f64| (50.0..1_500.0).contains(&h);
                    let key = |s: &(DVec3, f64, f64)| (good(s.1), (s.2 * 10.0).round(), s.1);
                    key(x).partial_cmp(&key(y)).unwrap()
                })
                .map(|(d, h, _)| (d, h))
                .unwrap();
            let up = rot * spot.0;
            let eye = center + up * (radius + spot.1.max(0.0) + altitude_m);
            let to_star = (star - eye).normalize();
            let star_flat = (to_star - up * to_star.dot(up)).normalize();
            let (st, ct) = turn_deg.to_radians().sin_cos();
            let ahead = up.cross(star_flat) * ct + star_flat * st;
            // Look below the horizon (which dips as the ground falls away).
            let dip = (radius / (radius + altitude_m)).acos();
            let pitch = dip + down_deg.to_radians();
            let look = ahead * pitch.cos() - up * pitch.sin();
            (eye, eye + look * 1_000.0, up)
        }
    }
}

/// Shoot one stop of the flight to `<out>/<name>.png`.
fn shoot_stop(stop: &Stop, size: (u32, u32), out: &std::path::Path) -> std::path::PathBuf {
    let started = std::time::Instant::now();
    let mut app = build(size);
    app.update();
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("no GPU: {}", d.0);
        std::process::exit(1);
    }
    let radius = app
        .world
        .resource::<SpaceState>()
        .body(stop.body)
        .expect("body")
        .radius_m;
    travel_to(&mut app.world, stop.body, 0.0, 0.0, radius).expect("travel");
    add_ground(&mut app, stop.body, true);
    app.update_n(2);
    let aim = |app: &mut App| {
        let (eye, at, up) = place(app, stop);
        let mut q = app
            .world
            .query_filtered::<&mut Transform, With<SimulationFocus>>();
        q.single_mut(&mut app.world).unwrap().translation = eye;
        let mut q = app.world.query_filtered::<&mut Transform, With<Camera>>();
        for mut tf in q.iter_mut(&mut app.world) {
            *tf = Transform::from_translation(eye).looking_at(at, up.as_vec3());
        }
    };
    let far = match stop.eye {
        Eye::Radii { distance_radii, .. } if distance_radii > 10.0 => 3.0 / distance_radii,
        _ => 1.0,
    };
    app.world.spawn((
        Camera {
            // As the showcase's camera, quick to adapt.
            exposure: match Exposure::default() {
                Exposure::Auto {
                    min_ev100,
                    max_ev100,
                    compensation,
                    ..
                } => Exposure::Auto {
                    min_ev100,
                    max_ev100,
                    speed: 40.0,
                    compensation,
                },
                e => e,
            },
            ..Camera::perspective(stop.fov * far as f32)
        },
        Transform::default(),
    ));
    aim(&mut app);
    app.update_n(2);
    aim(&mut app);
    settle(&mut app);
    // Aim again: the world may have shifted its origin meanwhile.
    aim(&mut app);
    // Between stars the volumetric fog keeps the ground's fog in its history for a few hundred
    // frames after the jump (the renderer sees no camera cut), so wait it out there.
    app.update_n(if matches!(stop.eye, Eye::Between { .. }) {
        300
    } else {
        12
    });
    let path = out.join(format!("{}.png", stop.name));
    match capture_now(&mut app.world, &path) {
        Ok(_) => println!(
            "{:<24} {:>6.1}s  -> {}",
            stop.name,
            started.elapsed().as_secs_f32(),
            path.display()
        ),
        Err(e) => eprintln!("{}: {e}", stop.name),
    }
    path
}

/// The flight's stops side by side, four to a row, in `<out>/flight.jpg`.
fn contact_sheet(shots: &[std::path::PathBuf], out: &std::path::Path) {
    let (w, h) = (480, 270);
    let rows = shots.len().div_ceil(4) as u32;
    let mut sheet = image::RgbImage::new(4 * w, rows * h);
    for (i, p) in shots.iter().enumerate() {
        let Ok(img) = image::open(p) else { continue };
        let small = image::imageops::thumbnail(&img.to_rgb8(), w, h);
        let (x, y) = ((i % 4) as i64 * w as i64, (i / 4) as i64 * h as i64);
        image::imageops::overlay(&mut sheet, &small, x, y);
    }
    if let Err(e) = sheet.save(out.join("flight.jpg")) {
        eprintln!("flight.jpg: {e}");
    }
}

fn main() {
    engine_core::log::init("warn");
    let args: Vec<String> = std::env::args().skip(1).collect();
    let out = std::path::PathBuf::from(
        args.first()
            .cloned()
            .unwrap_or_else(|| "approach_shots".into()),
    );
    let size = args
        .get(1)
        .and_then(|s| s.split_once('x'))
        .and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)))
        .unwrap_or((960, 540));
    let only: Vec<&str> = args.iter().skip(2).map(String::as_str).collect();
    std::fs::create_dir_all(&out).expect("output folder");
    // `flight [stops…]`: the flight from Earth's ground to Proxima b's, and a sheet of it.
    if only.first() == Some(&"flight") {
        let shots: Vec<_> = flight()
            .iter()
            .filter(|s| only.len() == 1 || only[1..].contains(&s.name))
            .map(|s| shoot_stop(s, size, &out))
            .collect();
        contact_sheet(&shots, &out);
        return;
    }
    for view in views() {
        if !only.is_empty() && !only.contains(&view.name) {
            continue;
        }
        for ground in [false, true] {
            shoot(&view, ground, size, &out);
        }
    }
}
