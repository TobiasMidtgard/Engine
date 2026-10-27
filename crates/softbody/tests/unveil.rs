//! The yard's unveiling, without drawing it: the crane lowers the silk sheet over the marble
//! statue, whose posed body is a triangle-mesh collider, and lifts it clear again.

use engine_assets::Assets;
use engine_core::prelude::*;
use engine_core::time::{FixedTime, TimeSource};
use engine_render::Mesh;
use engine_softbody::demo::body::BodyQuality;
use engine_softbody::demo::skin::BodyLibrary;
use engine_softbody::demo::statue::{UNVEIL_PERIOD, spawn_unveiling};
use engine_softbody::field::DistanceField;
use engine_softbody::{SoftBodyPlugin, SoftBodySim};
use engine_world::WeatherState;
use engine_world::physics::{Collider, ColliderShape};

fn find(app: &mut App, name: &str) -> Entity {
    let mut q = app.world.query::<(Entity, &Name)>();
    q.iter(&app.world)
        .find(|(_, n)| n.as_str() == name)
        .map(|(e, _)| e)
        .unwrap_or_else(|| panic!("no {name}"))
}

/// Run until `secs` of demo time have passed since the start.
fn run_to(app: &mut App, elapsed: &mut f32, secs: f32) {
    while *elapsed < secs {
        app.update();
        *elapsed += 1.0 / 60.0;
    }
}

#[test]
fn the_crane_drapes_the_statue_then_lifts_the_sheet_clear() {
    let mut app = App::new();
    app.add_plugins((engine_world::WorldPlugin, SoftBodyPlugin));
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 60.0,
        });
    app.world.resource_mut::<FixedTime>().set_hz(60.0);
    {
        let mut weather = app.world.resource_mut::<WeatherState>();
        weather.wind = Vec3::new(2.0, 0.0, 0.5);
        weather.gustiness = 0.0;
    }
    app.world.insert_resource(Assets::<Mesh>::default());
    {
        let mut library = app.world.resource_mut::<BodyLibrary>();
        library.blocking = true;
        library.quality = BodyQuality(2.0);
    }
    let facing = Vec3::new(0.5, 0.0, -0.87);
    app.add_systems(Startup, move |mut commands: Commands| {
        spawn_unveiling(&mut commands, DVec3::ZERO, facing);
    });
    let mut elapsed = 0.0;
    run_to(&mut app, &mut elapsed, 1.0);
    let body = find(&mut app, "Marble statue body");
    let sheet = find(&mut app, "Unveiling sheet");
    // The posed body became a fixed mesh collider.
    let (vertices, indices) = match &app.world.get::<Collider>(body).expect("cast").shape {
        ColliderShape::TriMesh { vertices, indices } => (vertices.clone(), indices.clone()),
        other => panic!("statue collider is {other:?}"),
    };
    assert!(indices.len() > 5000, "{} triangles", indices.len());
    let statue = *app.world.get::<GlobalTransform>(body).unwrap();
    let field = DistanceField::bake(&vertices, &indices, Vec3::ONE, 0.015).unwrap();
    let top = vertices.iter().fold(f32::MIN, |m, v| m.max(v.y)) as f64 + statue.translation().y;

    // Lowered and left on: the sheet hangs over the statue, through none of it.
    run_to(&mut app, &mut elapsed, UNVEIL_PERIOD * 0.72);
    let b = app.world.get::<SoftBodySim>(sheet).expect("sheet built");
    let inv = statue.affine().inverse();
    let mut deepest = 0.0f32;
    let mut low = 0;
    for i in 0..b.particle_count() {
        let p = b.world_position(i);
        assert!(p.is_finite(), "particle {i} exploded");
        let local = inv.transform_point3(p).as_vec3();
        if let Some((d, _)) = field.sample(local) {
            deepest = deepest.max(-d);
        }
        low += (p.y < top - 1.0) as usize;
    }
    assert!(
        deepest < 0.03,
        "the sheet sank {deepest:.3} m into the statue"
    );
    // The report the PC is asked for: the hook comes down onto the statue's crown, and the
    // statue is a baked mesh collider for the sheet.
    let report = engine_core::registry::call_api(
        &mut app.world,
        "softbody.unveil_report",
        serde_json::json!({}),
    )
    .unwrap();
    let sheet_report = &report["sheets"][0];
    assert_eq!(sheet_report["cast"], true, "{report}");
    assert!(sheet_report["beside_m"].as_f64().unwrap() < 0.1, "{report}");
    assert!(
        sheet_report["baked_fields"].as_u64().unwrap() >= 1,
        "{report}"
    );
    let b = app.world.get::<SoftBodySim>(sheet).unwrap();
    let covered = low as f32 / b.particle_count() as f32;
    assert!(
        covered > 0.4,
        "the sheet should hang down around the statue ({:.0} % over a meter below its head)",
        covered * 100.0
    );

    // Lifted off: the whole sheet hangs above the statue's head.
    run_to(&mut app, &mut elapsed, UNVEIL_PERIOD * 0.97);
    let b = app.world.get::<SoftBodySim>(sheet).unwrap();
    let lowest = (0..b.particle_count())
        .map(|i| b.world_position(i).y)
        .fold(f64::MAX, f64::min);
    assert!(
        lowest > top + 0.05,
        "the sheet still hangs to {lowest:.2} m, the statue is {top:.2} m tall"
    );
}
