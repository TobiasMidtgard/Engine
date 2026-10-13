//! Cloth drapes over triangle-mesh and height-grid colliders: the solver bakes meshes into
//! distance grids once a soft body comes near (big ones on a thread of their own) and samples
//! height grids directly.

use std::collections::HashMap;
use std::f32::consts::FRAC_PI_2;
use std::time::{Duration, Instant};

use engine_core::prelude::*;
use engine_core::time::{FixedTime, TimeSource};
use engine_softbody::settings::SoftBodyConfig;
use engine_softbody::systems::ColliderWorld;
use engine_softbody::{Cloth, ClothPins, SimSettings, SoftBodyPlugin, SoftBodySim};
use engine_world::WeatherState;
use engine_world::physics::{Collider, ColliderShape};

fn app() -> App {
    let mut app = App::new();
    app.add_plugins((engine_world::WorldPlugin, SoftBodyPlugin));
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 60.0,
        });
    app.world.resource_mut::<FixedTime>().set_hz(60.0);
    // Still air, so the sheets fall straight.
    let mut weather = app.world.resource_mut::<WeatherState>();
    weather.wind = Vec3::ZERO;
    weather.gustiness = 0.0;
    app
}

/// A unit sphere: an icosahedron split `levels` times, wound outward.
fn icosphere(levels: u32) -> (Vec<Vec3>, Vec<[u32; 3]>) {
    let t = (1.0 + 5f32.sqrt()) / 2.0;
    let mut verts: Vec<Vec3> = [
        [-1.0, t, 0.0],
        [1.0, t, 0.0],
        [-1.0, -t, 0.0],
        [1.0, -t, 0.0],
        [0.0, -1.0, t],
        [0.0, 1.0, t],
        [0.0, -1.0, -t],
        [0.0, 1.0, -t],
        [t, 0.0, -1.0],
        [t, 0.0, 1.0],
        [-t, 0.0, -1.0],
        [-t, 0.0, 1.0],
    ]
    .iter()
    .map(|p| Vec3::from_array(*p).normalize())
    .collect();
    let mut tris: Vec<[u32; 3]> = vec![
        [0, 11, 5],
        [0, 5, 1],
        [0, 1, 7],
        [0, 7, 10],
        [0, 10, 11],
        [1, 5, 9],
        [5, 11, 4],
        [11, 10, 2],
        [10, 7, 6],
        [7, 1, 8],
        [3, 9, 4],
        [3, 4, 2],
        [3, 2, 6],
        [3, 6, 8],
        [3, 8, 9],
        [4, 9, 5],
        [2, 4, 11],
        [6, 2, 10],
        [8, 6, 7],
        [9, 8, 1],
    ];
    for _ in 0..levels {
        let mut mid: HashMap<(u32, u32), u32> = HashMap::new();
        let mut split = |a: u32, b: u32, verts: &mut Vec<Vec3>| {
            *mid.entry((a.min(b), a.max(b))).or_insert_with(|| {
                verts.push(((verts[a as usize] + verts[b as usize]) * 0.5).normalize());
                verts.len() as u32 - 1
            })
        };
        let mut next = Vec::with_capacity(tris.len() * 4);
        for [a, b, c] in tris {
            let ab = split(a, b, &mut verts);
            let bc = split(b, c, &mut verts);
            let ca = split(c, a, &mut verts);
            next.extend([[a, ab, ca], [b, bc, ab], [c, ca, bc], [ab, bc, ca]]);
        }
        tris = next;
    }
    for t in &mut tris {
        let [a, b, c] = t.map(|i| verts[i as usize]);
        if (b - a).cross(c - a).dot(a + b + c) < 0.0 {
            t.swap(1, 2);
        }
    }
    (verts, tris)
}

/// A square silk sheet lying flat, centered on `at`, free to fall.
fn sheet(app: &mut App, at: DVec3, size: f32, res: u32) -> Entity {
    app.world
        .spawn((
            Transform::from_translation(at).with_rotation(Quat::from_rotation_x(-FRAC_PI_2)),
            Cloth {
                size: [size, size],
                resolution: [res, res],
                anchor: [0.5, 0.5],
                pins: ClothPins::None,
                mass_per_m2: 0.08,
                bend_compliance: 1e-2,
                sim: SimSettings {
                    damping: 0.3,
                    ..SimSettings::default()
                },
                ..Cloth::default()
            },
        ))
        .id()
}

fn mesh_ball(app: &mut App, levels: u32, center: DVec3, radius: f32) -> Entity {
    let (vertices, indices) = icosphere(levels);
    app.world
        .spawn((
            Transform::from_translation(center).with_scale(Vec3::splat(radius)),
            Collider {
                shape: ColliderShape::TriMesh { vertices, indices },
                ..Default::default()
            },
        ))
        .id()
}

/// Lowest particle, the center particle and the farthest any particle sinks into a ball.
fn settle_on_ball(
    app: &App,
    cloth: Entity,
    res: u32,
    center: DVec3,
    radius: f64,
) -> (f64, DVec3, f64) {
    let b = app.world.get::<SoftBodySim>(cloth).expect("cloth built");
    let n = b.particle_count();
    let mut lowest = f64::MAX;
    let mut deepest = 0.0f64;
    for i in 0..n {
        let p = b.world_position(i);
        assert!(p.is_finite(), "particle {i} exploded");
        lowest = lowest.min(p.y);
        deepest = deepest.max(radius - (p - center).length());
    }
    let mid = (res / 2 * res + res / 2) as usize;
    (lowest, b.world_position(mid), deepest)
}

#[test]
fn cloth_drapes_over_a_mesh_ball() {
    let mut app = app();
    let center = DVec3::new(0.0, 1.0, 0.0);
    mesh_ball(&mut app, 3, center, 0.5);
    let res = 25;
    let cloth = sheet(&mut app, DVec3::new(0.0, 1.8, 0.0), 1.6, res);
    app.update_n(240);
    let (lowest, top, deepest) = settle_on_ball(&app, cloth, res, center, 0.5);
    assert!(deepest < 0.012, "cloth sank {deepest:.3} m into the ball");
    assert!(
        (1.49..1.56).contains(&top.y) && top.x.abs() < 0.1 && top.z.abs() < 0.1,
        "the middle of the sheet should rest on the top of the ball, at {top}"
    );
    // The edges fall past the equator: the sheet hangs around the ball, not flat on it.
    assert!(lowest < 0.9, "lowest particle at {lowest:.2} m");
    assert!(lowest > 0.05, "the sheet slid off onto the ground");
    let fields = &app.world.resource::<ColliderWorld>().fields;
    assert_eq!(fields.counts(), (1, 0), "one mesh baked");
}

#[test]
fn a_big_mesh_bakes_in_the_background_then_holds_the_cloth() {
    let mut app = app();
    // Hold the cloth still until the grid is ready.
    app.world.resource_mut::<SoftBodyConfig>().enabled = false;
    let center = DVec3::new(0.0, 1.0, 0.0);
    // 20 480 triangles: too many to bake within a step.
    mesh_ball(&mut app, 5, center, 0.5);
    let res = 21;
    let cloth = sheet(&mut app, DVec3::new(0.0, 1.7, 0.0), 1.4, res);
    let started = Instant::now();
    let mut saw_baking = false;
    loop {
        app.update();
        let (ready, baking) = app.world.resource::<ColliderWorld>().fields.counts();
        saw_baking |= baking == 1;
        if ready == 1 {
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(300),
            "the mesh never finished baking"
        );
    }
    assert!(saw_baking, "a big mesh should bake on its own thread");
    app.world.resource_mut::<SoftBodyConfig>().enabled = true;
    app.update_n(240);
    let (_, top, deepest) = settle_on_ball(&app, cloth, res, center, 0.5);
    assert!(deepest < 0.012, "cloth sank {deepest:.3} m into the ball");
    assert!(
        (1.49..1.56).contains(&top.y),
        "middle of the sheet at {top}"
    );
}

#[test]
fn cloth_rests_on_a_height_grid() {
    let mut app = app();
    // A 6 × 6 m grid at y = 1 with a 0.6 m mound in the middle, turned a little.
    let n = 13u32;
    let heights: Vec<f32> = (0..n * n)
        .map(|k| {
            let (r, c) = ((k / n) as f32, (k % n) as f32);
            let (x, z) = (c * 0.5 - 3.0, r * 0.5 - 3.0);
            0.6 * (-(x * x + z * z) / 0.8).exp()
        })
        .collect();
    app.world.spawn((
        Transform::from_xyz(0.0, 1.0, 0.0).with_rotation(Quat::from_rotation_y(0.3)),
        Collider {
            shape: ColliderShape::Heightfield {
                heights,
                rows: n,
                cols: n,
                size: [6.0, 6.0],
            },
            ..Default::default()
        },
    ));
    let res = 21;
    let cloth = sheet(&mut app, DVec3::new(0.0, 2.2, 0.0), 2.0, res);
    app.update_n(240);
    let b = app.world.get::<SoftBodySim>(cloth).expect("cloth built");
    let lowest = (0..b.particle_count())
        .map(|i| b.world_position(i).y)
        .fold(f64::MAX, f64::min);
    let top = b.world_position((res / 2 * res + res / 2) as usize);
    assert!(
        lowest > 0.97,
        "cloth fell through the grid to {lowest:.2} m"
    );
    assert!(
        (1.58..1.66).contains(&top.y),
        "the middle should rest on the mound's top, at {top}"
    );
    // The sheet follows the mound down: its edges sit well below its middle.
    assert!(lowest < 1.3, "lowest particle at {lowest:.2} m");
}
