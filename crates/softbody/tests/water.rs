//! Cloth, rope and hair in water: dry cloth floats until it soaks through and sinks, the part
//! under the surface gets wet and the wet creeps a little way above it, currents carry what
//! is in the water along, rain soaks cloth and wind dries it, and cloth crossing the surface
//! pushes the water (`WaterDisturbance`).

use std::f32::consts::FRAC_PI_2;
use std::sync::Arc;

use engine_core::prelude::*;
use engine_core::time::{FixedTime, TimeSource};
use engine_softbody::{
    Cloth, ClothPins, HairGroom, HairStyle, SimSettings, SoftBodyPlugin, SoftBodySim,
};
use engine_world::animation::{BonePose, SkeletonPose};
use engine_world::{
    PrecipitationKind, Water, WaterDisturbance, WaterQuery, WaterSample, WeatherState,
};

/// The surface of the test pool, 3 m above its floor (the ground).
const LEVEL: f64 = 3.0;

/// Calm water 3 m deep over a 100 m square, flowing at `current`.
struct Pool {
    current: Vec3,
}

impl WaterQuery for Pool {
    fn sample(&self, p: DVec3) -> Option<WaterSample> {
        (p.x.abs() < 50.0 && p.z.abs() < 50.0).then_some(WaterSample {
            surface_height: LEVEL,
            depth: LEVEL as f32,
            velocity: self.current,
            normal: Vec3::Y,
        })
    }
}

fn app(current: Option<Vec3>) -> App {
    let mut app = App::new();
    app.add_plugins((engine_world::WorldPlugin, SoftBodyPlugin));
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 60.0,
        });
    app.world.resource_mut::<FixedTime>().set_hz(60.0);
    let mut weather = app.world.resource_mut::<WeatherState>();
    weather.wind = Vec3::ZERO;
    weather.gustiness = 0.0;
    weather.precipitation_mm_h = 0.0;
    weather.temperature_c = 15.0;
    if let Some(current) = current {
        app.world.insert_resource(Water(Arc::new(Pool { current })));
    }
    app
}

/// A square cotton sheet lying flat, centered on `at`, free to fall.
fn sheet(app: &mut App, at: DVec3) -> Entity {
    app.world
        .spawn((
            Transform::from_translation(at).with_rotation(Quat::from_rotation_x(-FRAC_PI_2)),
            Cloth {
                size: [1.2, 1.2],
                resolution: [17, 17],
                anchor: [0.5, 0.5],
                pins: ClothPins::None,
                mass_per_m2: 0.15,
                sim: SimSettings {
                    damping: 0.3,
                    ..SimSettings::default()
                },
                ..Cloth::default()
            },
        ))
        .id()
}

/// A 1.2 m cloth hanging from a rail along its top edge at `top`.
fn hanging(app: &mut App, top: DVec3) -> Entity {
    app.world
        .spawn((
            Transform::from_translation(top),
            Cloth {
                size: [0.8, 1.2],
                resolution: [12, 25],
                anchor: [0.5, 0.0],
                pins: ClothPins::TopEdge { every: 1 },
                mass_per_m2: 0.15,
                sim: SimSettings {
                    damping: 0.3,
                    ..SimSettings::default()
                },
                ..Cloth::default()
            },
        ))
        .id()
}

fn body(app: &App, e: Entity) -> &SoftBodySim {
    app.world.get::<SoftBodySim>(e).expect("built")
}

/// Mean position and mean wetness.
fn mean(app: &App, e: Entity) -> (DVec3, f32) {
    let b = body(app, e);
    let n = b.particle_count();
    let mut sum = DVec3::ZERO;
    for i in 0..n {
        let p = b.world_position(i);
        assert!(p.is_finite(), "particle {i} exploded");
        sum += p;
    }
    let wet = b.wet.iter().sum::<f32>() / b.wet.len().max(1) as f32;
    (sum / n as f64, wet)
}

#[test]
fn dry_cloth_floats_then_soaks_through_and_sinks() {
    let mut app = app(Some(Vec3::ZERO));
    let cloth = sheet(&mut app, DVec3::new(0.0, LEVEL + 0.3, 0.0));
    app.update_n(90);
    let (floating, wet) = mean(&app, cloth);
    assert!(
        (floating.y - LEVEL).abs() < 0.04,
        "dry cloth should lie on the surface, at {:.3} m",
        floating.y
    );
    assert!(
        wet > 0.02 && wet < 0.4,
        "it starts soaking from below: {wet:.2}"
    );
    // Soaking through, it sinks, slowly: water holds it back.
    let mut deepest_speed = 0.0f64;
    let mut last = floating.y;
    for _ in 0..12 * 4 {
        app.update_n(15);
        let (c, _) = mean(&app, cloth);
        deepest_speed = deepest_speed.max((last - c.y) * 4.0);
        last = c.y;
    }
    let (sunk, wet) = mean(&app, cloth);
    assert!(wet > 0.9, "soaked through: {wet:.2}");
    assert!(sunk.y < LEVEL - 1.0, "it should sink, at {:.2} m", sunk.y);
    assert!(
        deepest_speed < 1.2,
        "it sank at {deepest_speed:.2} m/s; water should slow it"
    );
}

#[test]
fn a_dipped_cloth_wets_below_the_waterline_and_a_little_above() {
    let mut app = app(Some(Vec3::ZERO));
    // The bottom half hangs in the water.
    let cloth = hanging(&mut app, DVec3::new(0.0, LEVEL + 0.6, 0.0));
    app.update_n(60 * 15);
    let b = body(&app, cloth);
    let (mut deep, mut near, mut high) = (Vec::new(), Vec::new(), Vec::new());
    for i in 0..b.particle_count() {
        let above = b.world_position(i).y - LEVEL;
        let w = b.wet[i];
        match above {
            a if a < -0.15 => deep.push(w),
            a if (0.0..0.05).contains(&a) => near.push(w),
            a if a > 0.3 => high.push(w),
            _ => {}
        }
    }
    let avg = |v: &[f32]| v.iter().sum::<f32>() / v.len().max(1) as f32;
    let (deep, near, high) = (avg(&deep), avg(&near), avg(&high));
    eprintln!("wet: under {deep:.2}, at the line {near:.2}, high {high:.2}");
    assert!(deep > 0.9, "under water it soaks through: {deep:.2}");
    assert!(
        near > 0.15 && near < deep,
        "the wet creeps just past the waterline: {near:.2}"
    );
    assert!(high < 0.05, "the top stays dry: {high:.2}");
    // Soaked, the hem hangs straight down in the water again rather than floating.
    let lowest = (0..b.particle_count())
        .map(|i| b.world_position(i).y)
        .fold(f64::MAX, f64::min);
    assert!(
        lowest < LEVEL - 0.45,
        "the soaked hem should hang in the water, lowest at {lowest:.2} m"
    );
}

#[test]
fn the_wind_only_blows_what_is_out_of_the_water() {
    // The same cloth hanging in a stiff breeze, in air and all but its top in still water.
    let mut hems = Vec::new();
    for water in [None, Some(Vec3::ZERO)] {
        let mut app = app(water);
        app.world.resource_mut::<WeatherState>().wind = Vec3::new(0.0, 0.0, 6.0);
        let cloth = hanging(&mut app, DVec3::new(0.0, LEVEL + 0.1, 0.0));
        app.update_n(60 * 10);
        let b = body(&app, cloth);
        // The lowest dozen particles: how far downwind the hem hangs.
        let mut low: Vec<DVec3> = (0..b.particle_count())
            .map(|i| b.world_position(i))
            .collect();
        low.sort_by(|a, c| a.y.total_cmp(&c.y));
        let hem = low[..12].iter().map(|p| p.z).sum::<f64>() / 12.0;
        hems.push(hem);
    }
    eprintln!(
        "hem {:.2} m downwind in air, {:.2} m in water",
        hems[0], hems[1]
    );
    assert!(
        hems[0] > 0.3,
        "in air the breeze blows the hem out: {hems:?}"
    );
    assert!(
        hems[1].abs() < 0.25 * hems[0],
        "in water the hem hangs nearly straight down: {hems:?}"
    );
}

#[test]
fn a_current_carries_floating_cloth_along() {
    let mut app = app(Some(Vec3::new(0.5, 0.0, 0.0)));
    let cloth = sheet(&mut app, DVec3::new(0.0, LEVEL + 0.1, 0.0));
    app.update_n(60 * 4);
    let (c, _) = mean(&app, cloth);
    assert!(
        c.x > 1.2 && c.z.abs() < 0.5,
        "the cloth should drift downstream (swaying a little in the eddies), at {c}"
    );
}

#[test]
fn water_only_acts_where_it_is() {
    // No water provider: the sheet falls to the ground and stays dry.
    let mut app = app(None);
    let cloth = sheet(&mut app, DVec3::new(0.0, LEVEL + 0.3, 0.0));
    app.update_n(150);
    let (c, wet) = mean(&app, cloth);
    assert!(
        c.y < 0.2,
        "with no water it falls to the ground: {:.2} m",
        c.y
    );
    assert_eq!(wet, 0.0);
}

#[test]
fn rain_soaks_cloth_and_wind_dries_it_faster() {
    let mut app = app(None);
    let cloth = hanging(&mut app, DVec3::new(0.0, 2.0, 0.0));
    {
        let mut w = app.world.resource_mut::<WeatherState>();
        w.precipitation_mm_h = 20.0;
        w.precipitation_kind = PrecipitationKind::Rain;
    }
    app.update_n(60 * 10);
    let (_, rained) = mean(&app, cloth);
    assert!(rained > 0.6, "ten seconds of heavy rain: {rained:.2}");
    app.world.resource_mut::<WeatherState>().precipitation_mm_h = 0.0;
    app.update_n(60 * 10);
    let (_, still) = mean(&app, cloth);
    app.world.resource_mut::<WeatherState>().wind = Vec3::new(8.0, 0.0, 0.0);
    app.update_n(60 * 10);
    let (_, windy) = mean(&app, cloth);
    let (dried_still, dried_windy) = (rained - still, still - windy);
    assert!(dried_still > 0.0, "it dries in still air");
    assert!(
        dried_windy > 2.0 * dried_still,
        "wind dries it faster: {dried_still:.3} still, {dried_windy:.3} windy"
    );
}

#[test]
fn hair_in_a_stream_soaks_and_streams_out_with_the_current() {
    let mut tips = Vec::new();
    for current in [Vec3::ZERO, Vec3::new(0.4, 0.0, 0.0)] {
        let mut app = app(Some(current));
        // A head just above the water, the long hair hanging into it.
        let head = app
            .world
            .spawn((
                Transform::default(),
                SkeletonPose {
                    names: Arc::from(vec!["head".to_string()]),
                    parents: Arc::from(vec![None]),
                    bones: vec![BonePose {
                        translation: Vec3::new(0.0, LEVEL as f32 + 0.2, 0.0),
                        ..BonePose::default()
                    }],
                },
            ))
            .id();
        let hair = app
            .world
            .spawn((
                Transform::default(),
                HairGroom::long_hair("head", HairStyle::long()),
                ChildOf(head),
            ))
            .id();
        app.update_n(60 * 4);
        let b = body(&app, hair);
        // The particles deepest in the water are the tips.
        let mut under: Vec<usize> = (0..b.particle_count())
            .filter(|&i| b.world_position(i).y < LEVEL - 0.05)
            .collect();
        assert!(under.len() > 20, "the hair should reach into the water");
        under.sort_by(|&a, &c| b.world_position(a).y.total_cmp(&b.world_position(c).y));
        under.truncate(20);
        let x = under.iter().map(|&i| b.world_position(i).x).sum::<f64>() / 20.0;
        let wet = under.iter().map(|&i| b.wet[i]).sum::<f32>() / 20.0;
        assert!(wet > 0.5, "the hair under water soaks: {wet:.2}");
        tips.push(x);
    }
    eprintln!(
        "tips at x {:.2} still, {:.2} in the current",
        tips[0], tips[1]
    );
    assert!(
        tips[1] - tips[0] > 0.08,
        "the current should sweep the tips downstream: {tips:?}"
    );
}

#[test]
fn a_silk_strip_streams_out_in_a_current_and_ripples() {
    let mut app = app(Some(Vec3::new(0.0, 0.0, 0.7)));
    // A 4 m strip tied by its upstream end to a pole across the stream, laid downstream on
    // the surface.
    let strip = app
        .world
        .spawn((
            Transform::from_xyz(0.0, LEVEL + 0.02, 0.0)
                .with_rotation(Quat::from_rotation_x(-FRAC_PI_2)),
            Cloth {
                size: [0.4, 4.0],
                resolution: [5, 41],
                anchor: [0.5, 0.0],
                pins: ClothPins::TopEdge { every: 1 },
                mass_per_m2: 0.06,
                sim: SimSettings {
                    damping: 0.1,
                    buoyancy: 1.05,
                    soak: 0.1,
                    ..SimSettings::default()
                },
                ..Cloth::default()
            },
        ))
        .id();
    app.update_n(60 * 8);
    let tip = |app: &App| {
        let b = body(app, strip);
        (0..b.particle_count())
            .map(|i| b.world_position(i))
            .max_by(|a, c| a.z.total_cmp(&c.z))
            .unwrap()
    };
    let start = tip(&app);
    assert!(
        start.z > 3.2,
        "the strip should stream out downstream, tip at {start}"
    );
    assert!(
        (start.y - LEVEL).abs() < 0.6,
        "it streams near the surface, tip at {start}"
    );
    // The eddies keep it moving.
    let mut wander = 0.0f64;
    for _ in 0..40 {
        app.update_n(3);
        let t = tip(&app);
        wander = wander.max(((t.x - start.x).powi(2) + (t.y - start.y).powi(2)).sqrt());
    }
    eprintln!("tip at {start}, wanders {wander:.3} m");
    assert!(
        wander > 0.03,
        "the tip should ripple in the current: {wander:.3} m"
    );
    assert!(wander < 1.5, "but not thrash: {wander:.3} m");
}

/// Water pushes sent since the last look: count, and the total mass × speed.
#[derive(Resource, Default)]
struct Pushes(usize, f32);

fn count_pushes(mut seen: ResMut<Pushes>, mut pushes: MessageReader<WaterDisturbance>) {
    for p in pushes.read() {
        assert!(p.position.is_finite() && p.velocity.is_finite() && p.mass_kg > 0.0);
        assert!(
            (p.position.y - LEVEL).abs() < 1e-3,
            "pushes are on the surface: {p:?}"
        );
        seen.0 += 1;
        seen.1 += p.mass_kg * p.velocity.length();
    }
}

#[test]
fn cloth_dropped_on_water_pushes_it_and_a_still_one_does_not() {
    let mut app = app(Some(Vec3::ZERO));
    app.init_resource::<Pushes>()
        .add_systems(Update, count_pushes);
    // Dropped from half a meter onto still water.
    sheet(&mut app, DVec3::new(0.0, LEVEL + 0.5, 0.0));
    app.update_n(60);
    let landing = std::mem::take(&mut *app.world.resource_mut::<Pushes>());
    eprintln!("landing: {} pushes, {:.3} kg·m/s", landing.0, landing.1);
    assert!(
        landing.0 >= 6,
        "landing on the water pushes it: {} pushes",
        landing.0
    );
    assert!(
        landing.0 <= 6 * 60,
        "a few pushes per step, not one per particle"
    );
    // Floating still, it leaves the surface alone.
    app.update_n(60 * 4);
    *app.world.resource_mut::<Pushes>() = Pushes::default();
    app.update_n(60);
    let resting = app.world.resource::<Pushes>();
    eprintln!("resting: {} pushes, {:.3} kg·m/s", resting.0, resting.1);
    assert!(
        resting.1 < 0.05 * landing.1,
        "a cloth lying still on still water barely pushes it: {:.3} vs {:.3}",
        resting.1,
        landing.1
    );
}
