use super::*;
use engine_core::time::TimeSource;
use engine_render::prelude::Camera;

fn app() -> App {
    let mut app = App::new();
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 60.0,
        });
    app.add_plugins(engine_world::WorldPlugin)
        .add_plugins(SpacePlugin)
        .init_resource::<Atmosphere>();
    let mut commands = app.world.commands();
    demo::spawn_solar_system(&mut commands);
    app.world.flush();
    app.world.spawn((
        Transform::from_xyz(0.0, 2.0, 0.0),
        SimulationFocus::default(),
        Velocity::default(),
    ));
    app.world.spawn((
        StarLight::default(),
        DirectionalLight::default(),
        Transform::default(),
    ));
    app.update();
    app
}

fn state(app: &App) -> &SpaceState {
    app.world.resource::<SpaceState>()
}

fn focus(app: &mut App) -> DVec3 {
    let mut q = app
        .world
        .query_filtered::<&Transform, With<SimulationFocus>>();
    q.single(&app.world).unwrap().translation
}

fn sun_dir(app: &mut App) -> Vec3 {
    let mut q = app.world.query_filtered::<&Transform, With<StarLight>>();
    -q.single(&app.world).unwrap().forward()
}

#[test]
fn bodies_sit_at_their_real_distances() {
    let app = app();
    let s = state(&app);
    let earth = s.body("Earth").unwrap();
    let moon = s.body("Moon").unwrap();
    let sun = s.body("Sun").unwrap();
    let d_sun = (earth.inertial_position - sun.inertial_position).length() / AU;
    assert!((0.98..1.02).contains(&d_sun), "{d_sun} AU");
    let d_moon = (moon.inertial_position - earth.inertial_position).length();
    assert!(
        (356_000_000.0..407_000_000.0).contains(&d_moon),
        "{d_moon} m"
    );
    // Earth is the anchor: its home spot is the world origin.
    assert_eq!(s.anchor.map(|a| s.bodies[a].name.as_str()), Some("Earth"));
    let home = s.surface_point("Earth", 40.0, 0.0, 60.0).unwrap();
    assert!(home.length() < 1e-3, "{home:?}");
}

#[test]
fn anchor_stays_put_while_the_sky_turns() {
    let mut app = app();
    let earth_tf = |app: &mut App| {
        let e = state(app).body("Earth").unwrap().entity;
        *app.world.get::<Transform>(e).unwrap()
    };
    let before = earth_tf(&mut app);
    let sun_before = sun_dir(&mut app);
    let moon_before = state(&app).body("Moon").unwrap().world_position;
    // Six hours later.
    app.world.resource_mut::<SpaceClock>().seconds += 6.0 * 3_600.0;
    app.update();
    let after = earth_tf(&mut app);
    assert_eq!(before.translation, after.translation);
    assert_eq!(before.rotation, after.rotation);
    let sun_after = sun_dir(&mut app);
    let turned = sun_before.angle_between(sun_after).to_degrees();
    assert!(
        (80.0..100.0).contains(&turned),
        "sun moved {turned}° in 6 h"
    );
    let moon_after = state(&app).body("Moon").unwrap().world_position;
    assert!((moon_after - moon_before).length() > 1.0e8);
    // Sunlight at 1 AU.
    let mut q = app.world.query::<&DirectionalLight>();
    let lux = q.single(&app.world).unwrap().illuminance;
    assert!((120_000.0..135_000.0).contains(&lux), "{lux} lux");
}

#[test]
fn gravity_points_down_at_home() {
    let app = app();
    let g = app
        .world
        .resource::<Gravity>()
        .at(DVec3::new(0.0, 2.0, 0.0));
    assert!((g.length() - 9.82).abs() < 0.03, "{g:?}");
    assert!(g.normalize().y < -0.9999);
}

#[test]
fn travel_to_the_moon_and_stand_on_it() {
    let mut app = app();
    let at = travel_to(&mut app.world, "Moon", 10.0, 20.0, 2.0).unwrap();
    assert_eq!(at, DVec3::new(0.0, 2.0, 0.0));
    app.update();
    let p = focus(&mut app);
    let s = state(&app);
    assert_eq!(s.anchor.map(|a| s.bodies[a].name.as_str()), Some("Moon"));
    let (lat, lon, h) = s.lat_lon_height("Moon", p).unwrap();
    assert!(
        (lat - 10.0).abs() < 1e-6 && (lon - 20.0).abs() < 1e-6,
        "{lat} {lon}"
    );
    assert!((h - 2.0).abs() < 1e-3, "{h}");
    // Moon gravity, pointing down.
    let g = app
        .world
        .resource::<Gravity>()
        .at(DVec3::new(0.0, 2.0, 0.0));
    assert!((g.length() - 1.62).abs() < 0.02, "{g:?}");
    assert!(g.normalize().y < -0.999);
    // No air on the Moon: the nearest atmosphere is Earth's, far away.
    let atmo = app.world.resource::<Atmosphere>();
    assert!(atmo.planet_center.length() > 3.0e8);
    // ...and nothing to breathe or fly in.
    let air = sample_air(&app, DVec3::new(0.0, 2.0, 0.0));
    assert_eq!(air.density, 0.0);
}

fn sample_air(app: &App, p: DVec3) -> engine_world::AirSample {
    let w = &app.world;
    w.resource::<Atmospheres>().sample(
        w.resource::<engine_world::WeatherState>(),
        w.resource::<Gravity>(),
        w.resource::<engine_world::WindField>(),
        p,
    )
}

#[test]
fn every_planet_has_its_own_air() {
    let mut app = app();
    // Earth keeps the weather-driven standard atmosphere.
    let home = sample_air(&app, DVec3::new(0.0, 2.0, 0.0));
    assert!((1.1..1.3).contains(&home.density), "{}", home.density);
    // Mars: about 1.6 % of Earth's.
    travel_to(&mut app.world, "Mars", 0.0, 0.0, 2.0).unwrap();
    app.update();
    let mars = sample_air(&app, DVec3::new(0.0, 2.0, 0.0));
    assert!((0.015..0.025).contains(&mars.density), "{}", mars.density);
    let list = &app.world.resource::<Atmospheres>().bodies;
    assert!(
        list.iter().any(|a| a.sea_level_density == 0.0),
        "airless bodies listed"
    );
}

#[test]
fn surface_to_surface_trip_keeps_millimeters() {
    let mut app = app();
    for (body, lat, lon) in [
        ("Mars", -4.5, 137.4),
        ("Moon", 0.67, 23.47),
        ("Earth", 40.0, 0.0),
    ] {
        travel_to(&mut app.world, body, lat, lon, 1.8).unwrap();
        app.update_n(3);
        let p = focus(&mut app);
        let (la, lo, h) = state(&app).lat_lon_height(body, p).unwrap();
        assert!((h - 1.8).abs() < 1e-3, "{body}: {h}");
        assert!((la - lat).abs() < 1e-7 && (lo - lon).abs() < 1e-7, "{body}");
    }
}

#[test]
fn flying_into_the_moons_reach_reanchors_on_it() {
    let mut app = app();
    let moon = state(&app).body("Moon").unwrap().clone();
    // Put the focus 20 000 km above the Moon, at rest relative to Earth's frame.
    let above = moon.world_position + (DVec3::ZERO - moon.world_position).normalize() * 2.0e7;
    {
        let mut q = app
            .world
            .query_filtered::<&mut Transform, With<SimulationFocus>>();
        q.single_mut(&mut app.world).unwrap().translation = above;
    }
    let inertial_before = state(&app).to_inertial(above);
    // Hold the clock so "the same true position" is exact.
    app.world.resource_mut::<SpaceClock>().paused = true;
    app.update();
    let s = state(&app).clone();
    assert_eq!(s.anchor.map(|a| s.bodies[a].name.as_str()), Some("Moon"));
    assert_eq!(s.switches, 1);
    // Same true position, now expressed around the Moon (home right below the focus).
    let p = focus(&mut app);
    let inertial_after = s.to_inertial(p);
    assert!(
        (inertial_after - inertial_before).length() < 1.0,
        "{}",
        (inertial_after - inertial_before).length()
    );
    assert!(p.x.abs() < 1.0 && p.z.abs() < 1.0, "{p:?}");
    // Earth's frame turns, so a point at rest there moves relative to the Moon.
    let mut q = app
        .world
        .query_filtered::<&Velocity, With<SimulationFocus>>();
    let v = q.single(&app.world).unwrap().linear;
    assert!(v.length() > 100.0, "{v:?}");
}

#[test]
fn warp_speeds_up_the_clock() {
    let mut app = app();
    app.world.resource_mut::<SpaceClock>().warp = 1000.0;
    let t0 = app.world.resource::<SpaceClock>().seconds;
    app.update_n(60);
    let dt = app.world.resource::<SpaceClock>().seconds - t0;
    assert!((dt - 1000.0).abs() < 20.0, "{dt}");
}

#[test]
fn apis_are_registered() {
    use engine_core::registry::call_api;
    let mut app = app();
    let out = call_api(&mut app.world, "space.bodies", serde_json::json!({})).unwrap();
    assert_eq!(out["anchor"], "Earth");
    assert!(out["bodies"].as_array().unwrap().len() >= 10);
    let err = call_api(
        &mut app.world,
        "space.travel",
        serde_json::json!({"body": "Pluto"}),
    )
    .unwrap_err();
    assert!(err.contains("Moon"), "{err}");
}

#[test]
fn climbing_to_orbit_stops_the_world_turning() {
    let mut app = app();
    app.world.resource_mut::<SpaceClock>().paused = true;
    let up = DVec3::new(0.0, 1_500_000.0, 0.0);
    {
        let mut q = app
            .world
            .query_filtered::<&mut Transform, With<SimulationFocus>>();
        q.single_mut(&mut app.world).unwrap().translation = up;
    }
    let before = state(&app).to_inertial(up);
    app.update();
    let frame = app.world.resource::<SpaceFrame>().clone();
    assert!(!frame.rotating);
    assert_eq!(frame.anchor, "Earth");
    let p = focus(&mut app);
    assert!((state(&app).to_inertial(p) - before).length() < 1e-3);
    // At rest on the turning Earth = moving east in the fixed frame.
    let mut q = app
        .world
        .query_filtered::<&Velocity, With<SimulationFocus>>();
    let v = q.single(&app.world).unwrap().linear;
    let expected = 7.292e-5 * 7.871e6 * 40f64.to_radians().cos();
    assert!((v.length() as f64 - expected).abs() < 2.0, "{v:?}");
    assert!(v.x > 0.0, "eastward: {v:?}");
    // Now Earth turns in the world.
    let earth = state(&app).body("Earth").unwrap().entity;
    let r0 = app.world.get::<Transform>(earth).unwrap().rotation;
    app.world.resource_mut::<SpaceClock>().paused = false;
    app.world.resource_mut::<SpaceClock>().seconds += 3_600.0;
    app.update();
    let r1 = app.world.get::<Transform>(earth).unwrap().rotation;
    assert!(r0.angle_between(r1).to_degrees() > 14.0);
    // Back down: the world turns with Earth again, home under the focus.
    {
        let mut q = app
            .world
            .query_filtered::<&mut Transform, With<SimulationFocus>>();
        let earth_pos = state(&app).body("Earth").unwrap().world_position;
        let down = earth_pos
            + (q.single(&app.world).unwrap().translation - earth_pos).normalize() * 6_372_000.0;
        q.single_mut(&mut app.world).unwrap().translation = down;
    }
    app.update();
    assert!(app.world.resource::<SpaceFrame>().rotating);
    let p = focus(&mut app);
    assert!(
        p.x.abs() < 1.0 && p.z.abs() < 1.0 && (p.y - 1_000.0).abs() < 50.0,
        "{p:?}"
    );
}

#[test]
fn far_bodies_get_spheres_rings_and_stay_visible() {
    let mut app = app();
    app.world.spawn((Camera::default(), Transform::default()));
    app.update_n(2);
    let mut q = app.world.query::<(&ChildOf, &BodyImpostor, &Transform)>();
    let impostors: Vec<_> = q
        .iter(&app.world)
        .map(|(p, i, t)| (p.parent(), *i, t.scale.x))
        .collect();
    let s = state(&app);
    let of = |name: &str| {
        let e = s.body(name).unwrap().entity;
        impostors
            .iter()
            .filter(|(p, ..)| *p == e)
            .collect::<Vec<_>>()
    };
    // One sphere per planet and moon, none for the Sun; Saturn also has its rings.
    assert!(of("Sun").is_empty());
    assert_eq!(of("Jupiter").len(), 1);
    assert_eq!(of("Saturn").len(), 2);
    assert_eq!(impostors.len(), 10);
    // Inside the body (under any terrain), and the Moon is big enough to be seen as it is.
    let moon = of("Moon")[0];
    assert!(moon.1.radius_m < 1_737_400.0 && moon.1.radius_m > 1_700_000.0);
    assert_eq!(moon.2, 1.0);
    // Jupiter, hundreds of millions of km away, is enlarged to a visible dot.
    let jupiter = of("Jupiter")[0];
    let d = (s.body("Jupiter").unwrap().world_position).length();
    let apparent = jupiter.1.radius_m * jupiter.2 as f64 / d;
    assert!(
        jupiter.2 > 1.0 && (apparent - 7e-4).abs() < 1e-5,
        "{apparent}"
    );
}

#[test]
fn ring_opacity_has_the_cassini_division() {
    let rings = BodyRings::default();
    let b_ring = look::ring_opacity(&rings, 100_000_000.0);
    let cassini = look::ring_opacity(&rings, 119_000_000.0);
    assert!(b_ring > 0.6 && cassini < 0.15, "{b_ring} {cassini}");
    assert_eq!(look::ring_opacity(&rings, 60_000_000.0), 0.0);
}
