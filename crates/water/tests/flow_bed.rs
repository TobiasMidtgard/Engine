//! A stream's bed learns what moving bodies stand in it, looking only where they are: a crate
//! dropped in parts the water, a crate lying still costs no more rays, a crate moved on frees
//! the water where it was, and fixed scenery in the stream costs nothing at all.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_water::flow::{FlowBed, FlowRegion, FlowSource};
use engine_water::{SharedWater, WaterPlugin};
use engine_world::physics::{
    Collider, ColliderShape, Hit, PhysicsQueries, PhysicsQuery, QueryFilter, RigidBody,
};
use engine_world::{SimulationFocus, SurfaceId, WorldPlugin};
use parking_lot::RwLock;

/// A box standing on the ground: (entity, center x/z, half size, height).
type StandingBox = (Entity, [f64; 2], f64, f64);

/// Boxes standing on flat ground at y = 0.
#[derive(Default)]
struct Scene {
    boxes: RwLock<Vec<StandingBox>>,
    rays: AtomicUsize,
    overlaps: AtomicUsize,
}

struct Shared(Arc<Scene>);

impl PhysicsQuery for Shared {
    fn raycast(&self, o: DVec3, dir: Vec3, max: f32, filter: &QueryFilter) -> Option<Hit> {
        self.0.rays.fetch_add(1, Ordering::Relaxed);
        for &(e, c, half, h) in self.0.boxes.read().iter() {
            if filter.exclude.contains(&e) || (o.x - c[0]).abs() > half || (o.z - c[1]).abs() > half
            {
                continue;
            }
            let distance = if dir.y < 0.0 { o.y - h } else { 0.0 };
            if (0.0..=max as f64).contains(&distance) && (dir.y < 0.0 || o.y < h) {
                return Some(Hit {
                    entity: e,
                    point: DVec3::new(o.x, if dir.y < 0.0 { h } else { o.y }, o.z),
                    normal: Vec3::Y,
                    distance: distance as f32,
                    surface: SurfaceId(0),
                });
            }
        }
        None
    }
    fn shapecast(
        &self,
        _: &ColliderShape,
        _: DVec3,
        _: Quat,
        _: Vec3,
        _: f32,
        _: &QueryFilter,
    ) -> Option<Hit> {
        None
    }
    fn overlap(
        &self,
        shape: &ColliderShape,
        p: DVec3,
        _: Quat,
        _: &QueryFilter,
        out: &mut Vec<Entity>,
    ) {
        self.0.overlaps.fetch_add(1, Ordering::Relaxed);
        let ColliderShape::Cuboid { half_extents } = shape else {
            return;
        };
        for &(e, c, half, _) in self.0.boxes.read().iter() {
            if (p.x - c[0]).abs() <= half + half_extents.x as f64
                && (p.z - c[1]).abs() <= half + half_extents.z as f64
            {
                out.push(e);
            }
        }
    }
    fn closest_point(&self, _: DVec3, _: f32, _: &QueryFilter) -> Option<Hit> {
        None
    }
}

fn app(scene: &Arc<Scene>) -> (App, Entity) {
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 30.0,
        });
    app.add_plugins((WorldPlugin, WaterPlugin));
    app.insert_resource(PhysicsQueries(Arc::new(Shared(scene.clone()))));
    app.world.spawn((
        Transform::from_translation(DVec3::new(20.0, 1.0, 10.0)),
        SimulationFocus::default(),
    ));
    // A flat channel 40 × 20 m, a spring filling it.
    let flow = app
        .world
        .spawn((
            Transform::default(),
            FlowRegion {
                cells: [80, 40],
                cell_m: 0.5,
                // Down 3 cm a meter along x.
                bed: FlowBed::Heights(
                    (0..80 * 40)
                        .map(|k| 1.2 - (k % 80) as f32 * 0.5 * 0.03)
                        .collect(),
                ),
                sources: vec![FlowSource {
                    at: [2.0, 10.0],
                    radius_m: 2.0,
                    rate_m3_s: 2.0,
                }],
                open_edges: true,
                prewarm_s: 60.0,
                ..Default::default()
            },
        ))
        .id();
    (app, flow)
}

fn standing(app: &App, flow: Entity, x: f64, z: f64) -> f32 {
    let w = app.world.resource::<SharedWater>().0.clone();
    let w = w.read();
    let sim = w.flows.iter().find(|f| f.entity == flow).unwrap();
    let (i, j) = ((x / 0.5) as usize, (z / 0.5) as usize);
    sim.standing[j * sim.nx + i]
}

#[test]
fn only_moving_bodies_are_looked_for_and_only_where_they_move() {
    let scene = Arc::new(Scene::default());
    let (mut app, flow) = app(&scene);
    // Fixed scenery standing in the stream: never rayed.
    let rock = app
        .world
        .spawn((
            Transform::from_translation(DVec3::new(30.0, 0.5, 10.0)),
            RigidBody::fixed(),
            Collider::cuboid(Vec3::splat(0.5)),
        ))
        .id();
    scene.boxes.write().push((rock, [30.0, 10.0], 0.5, 1.0));
    for _ in 0..10 {
        app.update();
    }
    assert_eq!(
        scene.rays.load(Ordering::Relaxed),
        0,
        "fixed scenery costs no rays"
    );
    assert_eq!(scene.overlaps.load(Ordering::Relaxed), 0);

    // A crate dropped in at (15, 10).
    let crate_ = app
        .world
        .spawn((
            Transform::from_translation(DVec3::new(15.0, 0.6, 10.0)),
            RigidBody::dynamic(),
            Collider::cuboid(Vec3::splat(0.6)),
        ))
        .id();
    scene.boxes.write().push((crate_, [15.0, 10.0], 0.6, 1.2));
    for _ in 0..4 {
        app.update();
    }
    assert!(
        (standing(&app, flow, 15.1, 10.1) - 1.2).abs() < 0.01,
        "the water parts round it: {}",
        standing(&app, flow, 15.1, 10.1)
    );
    assert_eq!(standing(&app, flow, 30.1, 10.1), f32::NEG_INFINITY);
    let rays = scene.rays.load(Ordering::Relaxed);
    assert!(rays > 0);
    // Lying still: nothing more to look at.
    for _ in 0..20 {
        app.update();
    }
    assert_eq!(
        scene.rays.load(Ordering::Relaxed),
        rays,
        "a still crate is not rayed again"
    );

    // Moved on to (25, 10): the water closes where it was.
    app.world.get_mut::<Transform>(crate_).unwrap().translation = DVec3::new(25.0, 0.6, 10.0);
    scene.boxes.write()[1].1 = [25.0, 10.0];
    for _ in 0..4 {
        app.update();
    }
    assert_eq!(standing(&app, flow, 15.1, 10.1), f32::NEG_INFINITY);
    assert!((standing(&app, flow, 25.1, 10.1) - 1.2).abs() < 0.01);
}

/// A settled stream past the flows' detail range holds still (no steps) and wakes when the
/// focus comes back near.
#[test]
fn a_settled_stream_far_off_dozes_and_wakes_when_the_focus_returns() {
    let scene = Arc::new(Scene::default());
    let (mut app, flow) = app(&scene);
    let state = |app: &App| {
        let w = app.world.resource::<SharedWater>().0.clone();
        let w = w.read();
        let sim = w.flows.iter().find(|f| f.entity == flow).unwrap();
        (sim.dozing, sim.resting, sim.version, sim.steady_s)
    };
    // Settling near the focus: it keeps stepping.
    for _ in 0..30 * 8 {
        app.update();
    }
    let (dozing, _, _, steady) = state(&app);
    assert!(
        !dozing && steady >= engine_water::flow::SETTLED_S,
        "steady {steady} s"
    );
    let range = *app.world.resource::<engine_world::SimulationRange>();
    let focus = app
        .world
        .query_filtered::<Entity, With<SimulationFocus>>()
        .single(&app.world)
        .unwrap();
    // Out past the detail range, still well inside the simulation range.
    let away = 20.0
        + 30.0
        + range.active_m * engine_water::sim::FLOW_DETAIL_SHARE * (1.0 + range.hysteresis)
        + 5.0;
    assert!(
        away < 20.0 + range.active_m,
        "detail {away} m vs active {} m",
        range.active_m
    );
    app.world.get_mut::<Transform>(focus).unwrap().translation.x = away;
    for _ in 0..5 {
        app.update();
    }
    let (dozing, resting, v0, _) = state(&app);
    assert!(dozing && !resting, "dozing {dozing}, resting {resting}");
    for _ in 0..30 {
        app.update();
    }
    assert_eq!(state(&app).2, v0, "a dozing stream takes no steps");
    // Back by the stream: it runs again.
    app.world.get_mut::<Transform>(focus).unwrap().translation.x = 20.0;
    for _ in 0..5 {
        app.update();
    }
    let (dozing, _, v1, _) = state(&app);
    assert!(
        !dozing && v1 > v0,
        "awake again: dozing {dozing}, version {v0} -> {v1}"
    );
}
