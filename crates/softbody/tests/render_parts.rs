//! Draw geometry built in parts (as the render prepare does, one run of bodies per task) and
//! joined matches geometry built in one go; quality presets scale the soft bodies' cost.

use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_softbody::render::Geometry;
use engine_softbody::{SoftBodySim, demo};

#[test]
fn geometry_built_in_parts_joins_into_the_same_buffers() {
    let mut app = App::new();
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 60.0,
        });
    app.add_plugins((engine_world::WorldPlugin, engine_softbody::SoftBodyPlugin));
    app.add_systems(Startup, |mut commands: Commands| {
        demo::spawn_demo(&mut commands, DVec3::ZERO);
    });
    app.update_n(2);
    let mut q = app.world.query::<&SoftBodySim>();
    let bodies: Vec<&SoftBodySim> = q.iter(&app.world).collect();
    assert!(
        bodies.iter().any(|b| b.strands.is_some()),
        "the demo has hair"
    );
    assert!(
        bodies.iter().any(|b| !b.tubes.is_empty()),
        "the demo has ropes"
    );

    let build = |geo: &mut Geometry, b: &SoftBodySim| {
        let base = b.sim.origin.as_vec3();
        let range = geo.add_body(b, base, 0.5, f32::INFINITY);
        let hair = geo.add_hair(b, base, 0.5, 0.7, 1.0);
        (range, hair.map(|h| h.instances))
    };
    let mut whole = Geometry::default();
    let expected: Vec<_> = bodies.iter().map(|b| build(&mut whole, b)).collect();

    let mut joined = Geometry::default();
    let mut got = Vec::new();
    for run in bodies.chunks(5) {
        let mut part = Geometry::default();
        part.material_base = joined.materials.len() as u32;
        part.hole_base = joined.holes.len() as u32;
        let built: Vec<_> = run.iter().map(|b| build(&mut part, b)).collect();
        let (index, strand) = joined.append(&part);
        got.extend(built.into_iter().map(|(r, h)| {
            (
                r.start + index..r.end + index,
                h.map(|h| h.start + strand..h.end + strand),
            )
        }));
    }
    assert_eq!(got, expected);
    assert_eq!(joined.vertices, whole.vertices);
    assert_eq!(joined.indices, whole.indices);
    assert_eq!(joined.materials, whole.materials);
    assert_eq!(joined.holes, whole.holes);
    assert_eq!(joined.points, whole.points);
    assert_eq!(joined.guides, whole.guides);
    assert_eq!(joined.grooms, whole.grooms);
    assert_eq!(joined.strands, whole.strands);
}

#[test]
fn quality_presets_set_substeps_and_the_half_rate_distance() {
    use engine_softbody::SoftBodyConfig;
    use engine_world::{QualityPreset, Scalability};
    let mut app = App::new();
    app.add_plugins((engine_world::WorldPlugin, engine_softbody::SoftBodyPlugin));
    app.update();
    assert_eq!(
        *app.world.resource::<SoftBodyConfig>(),
        SoftBodyConfig::default()
    );
    app.insert_resource(Scalability::preset(QualityPreset::Low));
    app.update();
    let config = *app.world.resource::<SoftBodyConfig>();
    assert_eq!(config.quality, 0.5);
    assert_eq!(config.lod_distance, 15.0);
}

#[test]
fn far_cloth_steps_every_fourth_fixed_step_and_is_drawn_moving_across_them() {
    use engine_world::FocusPoints;
    let mut app = App::new();
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 60.0,
        });
    app.add_plugins((engine_world::WorldPlugin, engine_softbody::SoftBodyPlugin));
    app.add_systems(Startup, |mut commands: Commands| {
        demo::spawn_demo(&mut commands, DVec3::ZERO);
    });
    app.add_systems(PreUpdate, |mut focus: ResMut<FocusPoints>| {
        focus.points.clear();
        focus.points.push(DVec3::new(100.0, 1.7, 0.0));
    });
    // A light breeze: enough to keep the banner moving (awake), too little for a far flag to
    // record and loop its flapping instead of stepping (`sway::MIN_WIND`).
    {
        let mut weather = app.world.resource_mut::<engine_world::WeatherState>();
        weather.wind = Vec3::new(0.4, 0.0, 0.0);
        weather.gustiness = 0.0;
    }
    app.update_n(12);
    let name = |n: &Name| n.as_str() == "Sun banner";
    let mut q = app.world.query::<(&Name, &SoftBodySim)>();
    let mut seen = Vec::new();
    for _ in 0..9 {
        app.update();
        let (_, b) = q.iter(&app.world).find(|(n, _)| name(n)).unwrap();
        seen.push((b.steps, b.lod_wait, b.draw_alpha(0.0)));
    }
    let (_, b) = q.iter(&app.world).find(|(n, _)| name(n)).unwrap();
    assert_eq!(b.lod_every, 4, "{seen:?}");
    let steps: Vec<u64> = seen.iter().map(|s| s.0).collect();
    assert_eq!(
        steps[8] - steps[0],
        2,
        "two steps in eight fixed steps (at whatever phase it starts): {seen:?}"
    );
    // Drawing moves on a quarter of the way each fixed step.
    for pair in seen.windows(2) {
        let (a, b) = (pair[0].2, pair[1].2);
        assert!(b == 0.0 || (b - a - 0.25).abs() < 1e-6, "{seen:?}");
    }
}
