use super::*;
use engine_core::time::TimeSource;
use engine_render::prelude::Camera;
use engine_world::WeatherState;

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
    // The trip went through one world shift, so every module moved its state with it.
    let shifts = app
        .world
        .resource::<bevy_ecs::message::Messages<engine_world::WorldShift>>();
    assert_eq!(shifts.len(), 1);
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
    // Earth keeps the weather-driven standard atmosphere, and its sky is Earth's.
    let home = sample_air(&app, DVec3::new(0.0, 2.0, 0.0));
    assert!((1.1..1.3).contains(&home.density), "{}", home.density);
    let sky = *app.world.resource::<Atmosphere>();
    let earth = Atmosphere::earth();
    assert_eq!(
        (
            sky.rayleigh_scattering,
            sky.mie_absorption,
            sky.ozone_absorption
        ),
        (
            earth.rayleigh_scattering,
            earth.mie_absorption,
            earth.ozone_absorption
        )
    );
    // Mars: about 1.6 % of Earth's.
    travel_to(&mut app.world, "Mars", 0.0, 0.0, 2.0).unwrap();
    app.update();
    let mars = sample_air(&app, DVec3::new(0.0, 2.0, 0.0));
    assert!((0.015..0.025).contains(&mars.density), "{}", mars.density);
    // Its sky is its own dusty air (butterscotch by day, blue around the setting sun), around
    // the planet under the pad.
    let sky = *app.world.resource::<Atmosphere>();
    let dusty = Atmosphere::mars();
    assert!(sky.enabled);
    assert_eq!(
        (
            sky.rayleigh_scattering,
            sky.mie_scattering,
            sky.mie_absorption,
            sky.ozone_absorption,
            sky.mie_g_offset,
            sky.thickness
        ),
        (
            dusty.rayleigh_scattering,
            dusty.mie_scattering,
            dusty.mie_absorption,
            dusty.ozone_absorption,
            dusty.mie_g_offset,
            dusty.thickness
        )
    );
    assert!(
        (sky.planet_radius - 3_389_500.0).abs() < 1.0,
        "{}",
        sky.planet_radius
    );
    let down = sky.planet_center.normalize();
    assert!(down.y < -0.999, "{:?}", sky.planet_center);
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
fn the_valley_waits_on_earth_while_you_visit_the_moon() {
    for keep in [true, false] {
        let mut app = app();
        app.world.resource_mut::<SpaceSettings>().keep_surfaces = keep;
        // A house in the valley and a cart rolling down the street.
        let house = app
            .world
            .spawn(Transform::from_xyz(120.0, 0.0, -340.0))
            .id();
        let turned = Quat::from_rotation_y(0.7);
        let cart = app
            .world
            .spawn((
                Transform::from_xyz(-50.0, 0.5, 20.0).with_rotation(turned),
                Velocity {
                    linear: Vec3::new(3.0, 0.0, 0.0),
                    angular: Vec3::new(0.0, 0.2, 0.0),
                },
            ))
            .id();
        travel_to(&mut app.world, "Moon", 0.0, 0.0, 2.0).unwrap();
        app.update();
        // Earth turns for an hour while we are away.
        app.world.resource_mut::<SpaceClock>().seconds += 3_600.0;
        app.update_n(2);
        travel_home(&mut app.world, 2.0);
        app.update();

        let house_at = app.world.get::<Transform>(house).unwrap().translation;
        let off = (house_at - DVec3::new(120.0, 0.0, -340.0)).length();
        if !keep {
            // Without it the house stays where the Moon's frame mapped it, kilometers off.
            assert!(off > 1_000.0, "{off}");
            continue;
        }
        assert!(off < 1e-3, "house {house_at:?}");
        let c = *app.world.get::<Transform>(cart).unwrap();
        assert!((c.translation - DVec3::new(-50.0, 0.5, 20.0)).length() < 1e-3);
        assert!(c.rotation.angle_between(turned) < 1e-5);
        let v = app.world.get::<Velocity>(cart).unwrap();
        assert!(
            (v.linear - Vec3::new(3.0, 0.0, 0.0)).length() < 1e-4,
            "{v:?}"
        );
        assert!((v.angular - Vec3::new(0.0, 0.2, 0.0)).length() < 1e-5);
        // Home is the frame the world started in, with the focus above the home spot.
        assert!((focus(&mut app) - DVec3::new(0.0, 2.0, 0.0)).length() < 1e-6);
        assert_eq!(
            *app.world.resource::<SpaceFrame>(),
            SpaceFrame {
                anchor: "Earth".into(),
                ..SpaceFrame::default()
            }
        );
        assert!(app.world.resource::<Parked>().bodies.is_empty());
    }
}

#[test]
fn orbit_and_landing_elsewhere_keep_the_valley_in_place() {
    let mut app = app();
    let house = app
        .world
        .spawn(Transform::from_xyz(120.0, 0.0, -340.0))
        .id();
    // A satellite far above stays in space with the world.
    let satellite = app.world.spawn(Transform::from_xyz(0.0, 3.0e6, 0.0)).id();
    let on_earth = |app: &App| {
        let p = app.world.get::<Transform>(house).unwrap().translation;
        state(app).lat_lon_height("Earth", p).unwrap()
    };
    let before = on_earth(&app);
    let set_focus = |app: &mut App, p: DVec3| {
        let mut q = app
            .world
            .query_filtered::<&mut Transform, With<SimulationFocus>>();
        q.single_mut(&mut app.world).unwrap().translation = p;
    };
    set_focus(&mut app, DVec3::new(0.0, 1_500_000.0, 0.0));
    app.update();
    assert!(!app.world.resource::<SpaceFrame>().rotating);
    let parked = &app.world.resource::<Parked>().bodies["Earth"];
    assert!(parked.iter().any(|p| p.entity == house));
    assert!(!parked.iter().any(|p| p.entity == satellite));
    // Two hours of Earth turning under the fixed world, then land wherever the focus is.
    app.world.resource_mut::<SpaceClock>().seconds += 7_200.0;
    app.update();
    let earth = state(&app).body("Earth").unwrap().world_position;
    let p = focus(&mut app);
    set_focus(&mut app, earth + (p - earth).normalize() * 6_372_000.0);
    app.update();
    assert!(app.world.resource::<SpaceFrame>().rotating);
    let after = on_earth(&app);
    assert!(
        (after.0 - before.0).abs() < 1e-7 && (after.1 - before.1).abs() < 1e-7,
        "{before:?} → {after:?}"
    );
    assert!((after.2 - before.2).abs() < 1e-3, "{before:?} → {after:?}");
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
    // The Encke gap is nearly empty, the A ring around it is not.
    assert!(look::ring_opacity(&rings, 133_580_000.0) < 0.05);
    assert!(look::ring_opacity(&rings, 128_000_000.0) > 0.35);
}

#[test]
fn bodies_shader_is_valid_wgsl() {
    let code = engine_render::compose_shader(look::BODIES_SHADER, true);
    engine_render::validate_wgsl("space bodies", &code).unwrap();
}

#[test]
fn far_looks_paint_what_photos_show() {
    let sys = system::solar_system();
    let body = |name: &str| sys.iter().find(|b| b.name == name).unwrap();
    let at =
        |name: &str, lat: f32, lon: f32| look::surface_color(body(name), look::look_dir(lat, lon));
    let luma = |c: [f32; 3]| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
    // Jupiter: the Great Red Spot is red, the North Tropical Zone brighter than the NEB.
    let grs = at("Jupiter", -22.4, look::GRS_LON_DEG);
    assert!(grs[0] > 1.6 * grs[1] && grs[0] > 2.0 * grs[2], "{grs:?}");
    assert!(luma(at("Jupiter", 21.0, 60.0)) > 1.5 * luma(at("Jupiter", 13.0, 60.0)));
    // Mars: rust at the equator, a white north polar cap.
    let mars = at("Mars", 0.0, 150.0);
    assert!(mars[0] > 2.0 * mars[2], "{mars:?}");
    assert!(luma(at("Mars", 89.0, 0.0)) > 0.6);
    // The Moon: Mare Imbrium is dark against the far side's highlands.
    assert!(luma(at("Moon", 33.0, -16.0)) < 0.6 * luma(at("Moon", 20.0, 170.0)));
    // Neptune is blue, Uranus cyan, Saturn butterscotch.
    let nep = at("Neptune", 10.0, 0.0);
    assert!(nep[2] > 2.0 * nep[0], "{nep:?}");
    let ura = at("Uranus", 10.0, 0.0);
    assert!(ura[2] > ura[0] && ura[1] > ura[0], "{ura:?}");
    let sat = at("Saturn", 10.0, 0.0);
    assert!(sat[0] > 1.3 * sat[2], "{sat:?}");
}

#[test]
fn earth_has_oceans_land_and_clouds() {
    let sys = system::solar_system();
    let earth = sys.iter().find(|b| b.name == "Earth").unwrap();
    let maps = look::bake_surface(earth, 128);
    assert!(maps.clouds && !maps.relief);
    assert_eq!(maps.albedo.width, 128);
    let texels = maps.albedo.data.len() / 4;
    let water = maps.albedo.data.chunks(4).filter(|p| p[3] > 128).count() as f32 / texels as f32;
    let cloud = maps
        .aux
        .data
        .chunks(4)
        .map(|p| p[0] as f32 / 255.0)
        .sum::<f32>()
        / texels as f32;
    assert!((0.45..0.85).contains(&water), "water {water}");
    assert!((0.25..0.8).contains(&cloud), "cloud {cloud}");
}

#[test]
fn rocky_maps_carry_relief_and_full_mips() {
    let sys = system::solar_system();
    let moon = sys.iter().find(|b| b.name == "Moon").unwrap();
    let maps = look::bake_surface(moon, 256);
    assert!(maps.relief && !maps.clouds);
    assert_eq!((maps.albedo.width, maps.albedo.height), (256, 128));
    assert!(maps.albedo.is_valid() && maps.aux.is_valid());
    assert_eq!(maps.albedo.mips.len(), 8);
    // Craters tilt the surface: the slopes are not all flat.
    let tilted = maps
        .aux
        .data
        .chunks(4)
        .filter(|p| p[1].abs_diff(128) > 6)
        .count();
    assert!(tilted > 500, "{tilted}");
}

/// Where the weather's formula puts a sun at `hour` local solar time on weather day `day` over
/// the home spot (40° N), in the planet's compass (east +X, north −Z).
fn weather_sun_at_home(hour: f64, day: f64) -> DVec3 {
    let tau = std::f64::consts::TAU;
    let lat = 40f64.to_radians();
    let decl = -23.44f64.to_radians() * (tau * (day + 10.0) / sun::YEAR_DAYS).cos();
    let h = (hour - 12.0) / 24.0 * tau;
    let up = lat.sin() * decl.sin() + lat.cos() * decl.cos() * h.cos();
    let east = -decl.cos() * h.sin();
    let north = lat.cos() * decl.sin() - lat.sin() * decl.cos() * h.cos();
    DVec3::new(east, up, -north).normalize()
}

fn degrees_apart(a: DVec3, b: Vec3) -> f64 {
    a.angle_between(b.as_dvec3().normalize()).to_degrees()
}

#[test]
fn starlight_tells_the_weather_where_the_sun_is() {
    let mut app = app();
    // J2000 is 2000-01-01 12:00: the weather's day 0.5.
    let day = app.world.resource::<WeatherState>().day_of_year as f64;
    let off = (day - 0.5 + 182.625).rem_euclid(365.25) - 182.625;
    assert!(off.abs() < 1.5, "epoch is day {day}");
    // At noon at the home spot (0° E), give or take the equation of time.
    let hour = app.world.resource::<WeatherState>().time_of_day_h;
    assert!((hour - 12.0).abs() < 0.25, "epoch is {hour} h");
    for (days, hours) in [(0.0, 3.3), (95.0, 7.0), (172.0, 15.5), (260.0, 11.0)] {
        app.world.resource_mut::<SpaceClock>().seconds = days * DAY + hours * 3_600.0;
        app.update();
        let light = sun_dir(&mut app);
        let w = app.world.resource::<WeatherState>().clone();
        assert!(degrees_apart(light.as_dvec3(), w.sun_direction) < 0.01);
        let formula = weather_sun_at_home(w.time_of_day_h as f64, w.day_of_year as f64);
        let apart = degrees_apart(formula, light);
        assert!(
            apart < 2.0,
            "day {days} +{hours} h: weather says {} h on day {}, {apart}° from the sun",
            w.time_of_day_h,
            w.day_of_year
        );
    }
}

#[test]
fn the_space_clock_can_follow_the_weathers() {
    let mut app = app();
    app.world.resource_mut::<SpaceSettings>().follow_world_clock = true;
    let set = |app: &mut App, hour: f32, day: f32| {
        let mut w = app.world.resource_mut::<WeatherState>();
        w.time_of_day_h = hour;
        w.day_of_year = day;
        w.sun_direction = Vec3::Y;
        for _ in 0..4 {
            app.update();
        }
        app.world.resource::<SpaceClock>().seconds
    };
    for (hour, day) in [(9.0, 172.0), (12.0, 355.0), (17.25, 80.0), (6.5, 300.0)] {
        set(&mut app, hour, day);
        let apart = degrees_apart(
            weather_sun_at_home(hour as f64, day as f64),
            sun_dir(&mut app),
        );
        assert!(apart < 2.0, "{hour} h on day {day}: {apart}° off");
        // The weather lights the scene: its sun stays its own.
        assert_eq!(app.world.resource::<WeatherState>().sun_direction, Vec3::Y);
    }
    // Midwinter noon: due south, 26.6° up.
    set(&mut app, 12.0, 355.0);
    let s = sun_dir(&mut app);
    assert!(s.x.abs() < 0.03 && s.z > 0.0, "{s:?}");
    assert!((s.y.asin().to_degrees() - 26.6).abs() < 1.0, "{s:?}");
    // Half an hour of weather is half an hour of space; the date turns over at midnight.
    let a = set(&mut app, 9.0, 40.0);
    let b = set(&mut app, 9.5, 40.0);
    assert!((b - a - 1_800.0).abs() < 5.0, "{}", b - a);
    let a = set(&mut app, 23.75, 40.0);
    let b = set(&mut app, 0.25, 41.0);
    assert!((b - a - 1_800.0).abs() < 5.0, "{}", b - a);
}

#[test]
fn a_turned_home_matches_the_weathers_compass() {
    let mut app = app();
    {
        let mut frame = app.world.resource_mut::<SpaceFrame>();
        frame.home_heading_deg = 180.0;
    }
    app.world.resource_mut::<SpaceSettings>().follow_world_clock = true;
    {
        let mut w = app.world.resource_mut::<WeatherState>();
        w.time_of_day_h = 8.0;
        w.day_of_year = 120.0;
    }
    for _ in 0..4 {
        app.update();
    }
    let s = state(&app);
    let home = s.surface_point("Earth", 40.0, 0.0, 60.0).unwrap();
    assert!(home.length() < 1e-3, "{home:?}");
    // North (toward the pole) is +Z now.
    let north = s.surface_point("Earth", 40.01, 0.0, 60.0).unwrap();
    assert!(north.z > 1_000.0 && north.x.abs() < 1.0, "{north:?}");
    // The weather's morning sun (north +Z, east −X) is where the real one stands.
    let ours = weather_sun_at_home(8.0, 120.0);
    let weathers = DVec3::new(-ours.x, ours.y, -ours.z);
    let apart = degrees_apart(weathers, sun_dir(&mut app));
    assert!(apart < 2.0, "{apart}°");
}

/// A free physics body `rel` meters from `body`'s center, moving `vel` m/s against it
/// (inertial axes), placed in the world as it is now.
fn launch(app: &mut App, body: &str, rel: DVec3, vel: DVec3) -> Entity {
    let s = state(app).clone();
    let b = s.body(body).unwrap();
    let (inertial, moving) = (b.inertial_position + rel, b.inertial_velocity + vel);
    let frame = app.world.resource::<SpaceFrame>().clone();
    let here = FrameRef {
        anchor: &s.bodies[s.anchor.unwrap()],
        rotating: frame.rotating,
        map: s.world_from_inertial,
    };
    let rot = s.world_from_inertial.0;
    let v = rot * (moving - here.velocity_at(inertial));
    app.world
        .spawn((
            Transform {
                translation: s.to_world(inertial),
                ..Default::default()
            },
            Velocity {
                linear: v.as_vec3(),
                angular: Vec3::ZERO,
            },
            engine_world::physics::RigidBody::dynamic(),
        ))
        .id()
}

/// Where a body is against `body`'s center now: distance and inertial velocity.
fn against(app: &mut App, e: Entity, body: &str) -> (f64, DVec3) {
    let s = state(app).clone();
    let b = s.body(body).unwrap();
    let p = app.world.get::<Transform>(e).unwrap().translation;
    let v = app.world.get::<Velocity>(e).unwrap().linear.as_dvec3();
    let frame = app.world.resource::<SpaceFrame>().clone();
    let here = FrameRef {
        anchor: &s.bodies[s.anchor.unwrap()],
        rotating: frame.rotating,
        map: s.world_from_inertial,
    };
    let inertial = s.to_inertial(p);
    let moving = s.world_from_inertial.0.inverse() * v + here.velocity_at(inertial);
    (
        (inertial - b.inertial_position).length(),
        moving - b.inertial_velocity,
    )
}

fn rails_of(app: &App, e: Entity) -> Option<OnRails> {
    app.world.get::<OnRails>(e).cloned()
}

#[test]
fn a_ship_in_low_orbit_coasts_through_warp_and_keeps_its_motion() {
    let mut app = app();
    let r = 6_371_000.0 + 400_000.0;
    let mu = state(&app).body("Earth").unwrap().mu;
    let speed = (mu / r).sqrt();
    let ship = launch(
        &mut app,
        "Earth",
        DVec3::new(r, 0.0, 0.0),
        DVec3::new(0.0, 0.0, -speed),
    );
    // Things on the ground, and things the game keeps off the rails, stay with physics.
    let crate_on_ground = app
        .world
        .spawn((
            Transform::from_xyz(3.0, 1.0, 0.0),
            Velocity::default(),
            engine_world::physics::RigidBody::dynamic(),
        ))
        .id();
    let kept = launch(
        &mut app,
        "Earth",
        DVec3::new(0.0, 0.0, r),
        DVec3::new(speed, 0.0, 0.0),
    );
    app.world.entity_mut(kept).insert(NoRails);

    app.world.resource_mut::<SpaceClock>().warp = 1_000.0;
    let t0 = app.world.resource::<SpaceClock>().seconds;
    for _ in 0..200 {
        app.update();
        let (d, _) = against(&mut app, ship, "Earth");
        assert!((d - r).abs() < 5.0, "altitude drifted by {} m", d - r);
    }
    let rails = rails_of(&app, ship).expect("on rails");
    assert_eq!(rails.around, "Earth");
    assert!(rails_of(&app, crate_on_ground).is_none());
    assert!(rails_of(&app, kept).is_none());
    // It went round as far as the clock says (200 frames of 16.7 s: about 0.6 of a turn).
    let elapsed = app.world.resource::<SpaceClock>().seconds - t0;
    let (p, _) = rails.conic.state_at(rails.conic.epoch);
    let turned = DVec3::new(r, 0.0, 0.0).angle_between(p);
    let expected = (elapsed * speed / r).rem_euclid(std::f64::consts::TAU);
    let expected = expected.min(std::f64::consts::TAU - expected);
    assert!((turned - expected).abs() < 1e-3, "{turned} vs {expected}");

    // Back to real time: physics takes it with the motion it had.
    app.world.resource_mut::<SpaceClock>().warp = 1.0;
    app.update();
    assert!(rails_of(&app, ship).is_none());
    let (d, v) = against(&mut app, ship, "Earth");
    assert!((d - r).abs() < 5.0);
    assert!(
        (v.length() - speed).abs() < 0.05,
        "{} vs {speed}",
        v.length()
    );
    assert!(v.dot(p.normalize()).abs() < 0.5, "still circling");
}

#[test]
fn a_ship_sent_past_the_moon_swings_by_it() {
    let mut app = app();
    let (moon_mu, moon_soi) = {
        let m = state(&app).body("Moon").unwrap();
        (m.mu, m.soi_m)
    };
    // Just outside the Moon's reach, falling toward it at 1 km/s, aimed 3,000 km off its center.
    let out = DVec3::new(0.3, 0.1, 1.0).normalize();
    let side = out.cross(DVec3::Y).normalize();
    let start = moon_soi + 4_000_000.0;
    let rp = 3_000_000.0;
    let v_inf2 = 1_000.0f64.powi(2) - 2.0 * moon_mu / start;
    let vp = (v_inf2 + 2.0 * moon_mu / rp).sqrt();
    let across = rp * vp / start;
    let moon = state(&app).body("Moon").unwrap().clone();
    let earth = state(&app).body("Earth").unwrap().clone();
    let rel = moon.inertial_position - earth.inertial_position + out * start;
    let vel = moon.inertial_velocity - earth.inertial_velocity
        + (-out * (1_000.0f64.powi(2) - across * across).sqrt() + side * across);
    let ship = launch(&mut app, "Earth", rel, vel);

    app.world.resource_mut::<SpaceClock>().warp = 5_000.0;
    let mut closest = f64::INFINITY;
    let mut around_moon = false;
    for _ in 0..1_500 {
        app.update();
        let (d, _) = against(&mut app, ship, "Moon");
        closest = closest.min(d);
        around_moon |= rails_of(&app, ship).is_some_and(|r| r.around == "Moon");
    }
    assert!(around_moon, "never entered the Moon's sphere of influence");
    assert!(app.world.resource::<RailsLog>().handoffs >= 1);
    // Sampled every 83 s near 2 km/s, so the closest frame is within ~100 km of periapsis.
    assert!(
        (closest - rp).abs() < 150_000.0,
        "closest {} km",
        closest / 1000.0
    );
    assert_eq!(app.world.resource::<RailsLog>().warp_drops, 0);
}

#[test]
fn a_ship_coasting_into_the_ground_drops_the_warp_above_it() {
    let mut app = app();
    let (moon_soi, moon_r) = {
        let m = state(&app).body("Moon").unwrap();
        (m.soi_m, m.radius_m)
    };
    let moon = state(&app).body("Moon").unwrap().clone();
    let earth = state(&app).body("Earth").unwrap().clone();
    let out = DVec3::new(-0.2, 0.3, 1.0).normalize();
    let rel = moon.inertial_position - earth.inertial_position + out * (moon_soi * 0.8);
    let vel = moon.inertial_velocity - earth.inertial_velocity - out * 1_500.0;
    let ship = launch(&mut app, "Earth", rel, vel);
    app.world.resource_mut::<SpaceClock>().warp = 5_000.0;
    let mut frames = 0;
    while app.world.resource::<SpaceClock>().warp > 1.0 && frames < 2_000 {
        app.update();
        frames += 1;
    }
    assert_eq!(
        app.world.resource::<SpaceClock>().warp,
        1.0,
        "warp never dropped"
    );
    assert!(rails_of(&app, ship).is_none());
    assert_eq!(app.world.resource::<RailsLog>().warp_drops, 1);
    // Handed to physics at the floor of low space (0.8% of the radius up), falling fast.
    let (d, v) = against(&mut app, ship, "Moon");
    let floor = moon_r * 1.008;
    assert!(
        (d - floor).abs() < 2_000.0,
        "{} km up",
        (d - moon_r) / 1000.0
    );
    assert!(v.length() > 2_000.0, "{} m/s", v.length());
}

#[test]
fn a_ship_past_escape_speed_leaves_for_the_sun() {
    let mut app = app();
    let r = 6_371_000.0 + 300_000.0;
    // Straight out of the ecliptic, well clear of the Moon.
    let ship = launch(
        &mut app,
        "Earth",
        DVec3::new(r, 0.0, 0.0),
        DVec3::new(0.0, 12_000.0, 0.0),
    );
    app.world.resource_mut::<SpaceClock>().warp = 100_000.0;
    for _ in 0..300 {
        app.update();
    }
    let rails = rails_of(&app, ship).expect("on rails");
    assert_eq!(rails.around, "Sun");
    let (d, _) = against(&mut app, ship, "Earth");
    let soi = state(&app).body("Earth").unwrap().soi_m;
    assert!(d > soi, "{} km from Earth", d / 1000.0);
    let out = engine_core::registry::call_api(&mut app.world, "space.rails", serde_json::json!({}))
        .unwrap();
    assert_eq!(out["ships"][0]["around"], "Sun");
}

#[test]
fn the_world_follows_a_coasting_ship_into_the_moons_frame() {
    let mut app = app();
    let (moon, earth) = {
        let s = state(&app);
        (
            s.body("Moon").unwrap().clone(),
            s.body("Earth").unwrap().clone(),
        )
    };
    let out = DVec3::new(-0.4, 0.05, 1.0).normalize();
    let side = out.cross(DVec3::Y).normalize();
    let rel = moon.inertial_position - earth.inertial_position + out * (moon.soi_m * 1.05);
    let vel = moon.inertial_velocity - earth.inertial_velocity - out * 900.0 + side * 60.0;
    let ship = launch(&mut app, "Earth", rel, vel);
    app.world
        .entity_mut(ship)
        .insert(SimulationFocus { priority: 10 });
    app.world.resource_mut::<SpaceClock>().warp = 2_000.0;
    let mut last: Option<f64> = None;
    let mut anchored_on_moon = false;
    for _ in 0..1_200 {
        app.update();
        let (d, v) = against(&mut app, ship, "Moon");
        // Smooth through every re-anchoring: no jumps beyond a frame's flight.
        if let Some(l) = last {
            let step = v.length() * 2_000.0 / 60.0;
            assert!(
                (d - l).abs() <= step * 1.05 + 10.0,
                "jumped {} km",
                (d - l) / 1e3
            );
        }
        last = Some(d);
        anchored_on_moon |= state(&app)
            .anchor
            .map(|a| state(&app).bodies[a].name.as_str())
            == Some("Moon");
        assert!(d > moon.radius_m, "fell through the Moon");
    }
    assert!(anchored_on_moon, "the world never re-anchored on the Moon");
    assert!(
        rails_of(&app, ship).is_some_and(|r| r.around == "Moon")
            || app.world.resource::<RailsLog>().warp_drops == 1
    );
}
