//! Headless pictures of planets as a ship closes in on them, with the space module's far view
//! and the terrain module's ground drawn together the way the showcase draws them: to check
//! that the handover from one to the other can't be seen.
//!
//! ```text
//! cargo run --release -p approach_shots -- [out_dir] [WxH] [names…]
//! ```
//!
//! Each view is shot twice, `<name>_far.png` with the far view alone and `<name>_ground.png`
//! with the planet's terrain too, from the same place. Both are set up as the showcase sets
//! them up (games/showcase/src/planet.rs): an Earth-like world's far view painted from its
//! terrain's ground, the terrain wearing the far view's look from high up, the cloud tops kept
//! over the ground.

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
    for view in views() {
        if !only.is_empty() && !only.contains(&view.name) {
            continue;
        }
        for ground in [false, true] {
            shoot(&view, ground, size, &out);
        }
    }
}
