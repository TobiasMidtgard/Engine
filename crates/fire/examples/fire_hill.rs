//! A grass fire on a hillside: lit low on a dry slope in calm air, it races up toward the
//! ridge and barely creeps back down. Renders it at a few moments into PNGs, without a window.
//!
//! ```text
//! cargo run --release -p engine_fire --example fire_hill -- [out_dir] [stops] [width] [height]
//! ```
//!
//! `stops` is a comma-separated list of `view@secs` (seconds after the fire is lit; default:
//! every view at its own moments). The hill is a `Ground` the fire, the grass and the painted
//! ground all follow, so this is also the check that a meadow drapes over a slope. Needs a GPU
//! or a software Vulkan driver (lavapipe). Time runs in fixed 1/30 s steps, so the pictures
//! are the same every run.

use std::sync::Arc;

use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_fire::{
    FirePlugin, FireSettings, FuelPatch, GrassMeadow, GroundKind, GroundPaint, PaintedGround,
    PatchShape,
};
use engine_render::prelude::*;
use engine_render::{Mesh, RenderDisabled, Vertex, capture_now};
use engine_world::{
    Ground, GroundQuery, GroundSample, IgniteRequest, SurfaceId, WeatherState, WorldPlugin,
};

/// Height of the hillside at world X/Z: a valley floor at 0 rising to a ridge 22 m up,
/// steepest (about 25°) at x = 5, with gentle swells across it.
fn hill(x: f64, z: f64) -> f64 {
    let rise = 0.5 + 0.5 * ((x - 5.0) / 24.0).tanh();
    22.0 * rise + 1.4 * (z * 0.09 + 0.7).sin() * rise * (1.0 - rise) * 4.0
}

struct Hill;

impl GroundQuery for Hill {
    fn sample(&self, p: DVec3) -> Option<GroundSample> {
        let e = 0.25;
        let dx = (hill(p.x + e, p.z) - hill(p.x - e, p.z)) / (2.0 * e);
        let dz = (hill(p.x, p.z + e) - hill(p.x, p.z - e)) / (2.0 * e);
        Some(GroundSample {
            height: hill(p.x, p.z),
            normal: Vec3::new(-dx as f32, 1.0, -dz as f32).normalize(),
            surface: SurfaceId(3),
        })
    }
}

/// The land as a mesh (a little below the `Ground`, so the painted ground lies on top).
fn hill_mesh() -> Mesh {
    let (x0, x1, z0, z1, step) = (-220.0, 260.0, -240.0, 240.0, 2.0);
    let nx = ((x1 - x0) / step) as u32 + 1;
    let nz = ((z1 - z0) / step) as u32 + 1;
    let mut vertices = Vec::with_capacity((nx * nz) as usize);
    for j in 0..nz {
        for i in 0..nx {
            let x = x0 + i as f64 * step;
            let z = z0 + j as f64 * step;
            vertices.push(Vertex::new(
                Vec3::new(x as f32, (hill(x, z) - 0.08) as f32, z as f32),
                Vec3::Y,
                Vec2::new(x as f32, z as f32) * 0.25,
            ));
        }
    }
    let mut indices = Vec::with_capacity(((nx - 1) * (nz - 1) * 6) as usize);
    for j in 0..nz - 1 {
        for i in 0..nx - 1 {
            let a = j * nx + i;
            let (b, c, d) = (a + 1, a + nx, a + nx + 1);
            indices.extend_from_slice(&[a, c, b, b, c, d]);
        }
    }
    let mut mesh = Mesh::new(vertices, indices);
    mesh.compute_normals();
    mesh
}

struct Stop {
    key: &'static str,
    name: &'static str,
    eye: [f64; 3],
    target: [f64; 3],
}

const VIEWS: &[Stop] = &[
    Stop {
        key: "hill_side",
        name: "The slope in profile: the burn runs up it",
        eye: [-8.0, 8.5, -33.0],
        target: [-6.0, 8.0, 6.0],
    },
    Stop {
        key: "hill_foot",
        name: "From the foot of the hill, looking up after the fire",
        eye: [-36.0, 2.6, -7.0],
        target: [-6.0, 9.0, 1.0],
    },
    Stop {
        key: "hill_front",
        name: "The head of the fire climbing",
        eye: [-24.0, 6.5, -24.0],
        target: [-4.0, 8.5, 1.0],
    },
];

/// Default stops: (view, seconds after ignition).
const STOPS: &[(&str, f32)] = &[
    ("hill_side", 20.0),
    ("hill_front", 30.0),
    ("hill_foot", 40.0),
    ("hill_side", 45.0),
    ("hill_side", 75.0),
];

/// Where the fire is lit: low on the slope.
const IGNITE: [f64; 2] = [-20.0, 0.0];
/// Hour of the pictures: late afternoon, the sun low across the slope.
const HOUR: f32 = 16.5;

/// Unit vector toward the sun at `hour` (latitude 35°, midsummer, east = −X, north = +Z).
fn sun_direction(hour: f32) -> Vec3 {
    let (lat, decl) = (35.0f32.to_radians(), 23.44f32.to_radians());
    let h = (hour - 12.0) / 24.0 * std::f32::consts::TAU;
    let up = lat.sin() * decl.sin() + lat.cos() * decl.cos() * h.cos();
    let east = -decl.cos() * h.sin();
    let north = lat.cos() * decl.sin() - lat.sin() * decl.cos() * h.cos();
    Vec3::new(-east, up, north).normalize_or(Vec3::Y)
}

/// Put `schedule` in as the render schedule and return the one it replaces.
fn swap_render(app: &mut App, schedule: Schedule) -> Schedule {
    let mut schedules = app.world.resource_mut::<Schedules>();
    let old = schedules.remove(Render).expect("render schedule");
    schedules.insert(schedule);
    old
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let out = args
        .get(1)
        .cloned()
        .unwrap_or_else(|| "screenshots/fire".into());
    let mut stops: Vec<(&Stop, f32)> = match args.get(2).filter(|s| !s.is_empty()) {
        Some(list) => list
            .split(',')
            .filter_map(|s| {
                let (key, at) = s.split_once('@')?;
                Some((VIEWS.iter().find(|v| v.key == key)?, at.parse().ok()?))
            })
            .collect(),
        None => STOPS
            .iter()
            .filter_map(|&(key, at)| Some((VIEWS.iter().find(|v| v.key == key)?, at)))
            .collect(),
    };
    stops.sort_by(|a, b| a.1.total_cmp(&b.1));
    let width: u32 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(1280);
    let height: u32 = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(720);
    std::fs::create_dir_all(&out).expect("output directory");

    let started = std::time::Instant::now();
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = width;
        w.height = height;
    }
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 30.0,
        });
    app.add_plugins((RenderPlugin, WorldPlugin, FirePlugin));
    app.world.insert_resource(Ground(Arc::new(Hill)));
    // Only the meadow burns (not the whole valley), in calm air: the slope alone drives it.
    app.world.resource_mut::<FireSettings>().ground_fuel = false;
    {
        let mut weather = app.world.resource_mut::<WeatherState>();
        weather.wind = Vec3::new(0.0, 0.0, 0.4);
        weather.time_of_day_h = HOUR;
        weather.sun_direction = sun_direction(HOUR);
    }

    let mesh = app
        .world
        .resource_mut::<engine_assets::Assets<Mesh>>()
        .add(hill_mesh());
    let w = &mut app.world;
    w.spawn((
        Transform::default(),
        MeshRenderer::new(mesh, StandardMaterial::hex(0x6e6040).with_roughness(0.95)),
    ));
    // Pasture litter over the whole hill, with a dirt track along its foot.
    let center = DVec3::new(2.0, hill(2.0, 0.0), 0.0);
    let half = [44.0f32, 36.0];
    w.spawn((
        Transform::from_translation(center),
        PaintedGround {
            half_extents: [110.0, 110.0],
            base: GroundKind::Litter,
            edge_m: 0.0,
            paints: vec![
                GroundPaint::new(
                    GroundKind::Dirt,
                    [-42.0 - center.x as f32, 0.0],
                    PatchShape::Rect {
                        half_extents: [2.2, 110.0],
                    },
                )
                .with_edge(1.5, 1.2)
                .with_strength(0.8),
            ],
            ..Default::default()
        },
    ));
    // The dry meadow that burns, and thinner grass around it out to the hill's shoulders.
    w.spawn((
        Transform::from_translation(center),
        GrassMeadow {
            half_extents: half,
            density: 120.0,
            height: [0.35, 0.85],
            seed: 0x4111,
            ..Default::default()
        },
        FuelPatch::grass(PatchShape::Rect { half_extents: half }),
    ));
    for (i, (x, z, hx, hz)) in [
        (2.0, -66.0, 80.0, 30.0),
        (2.0, 66.0, 80.0, 30.0),
        (66.0, 0.0, 20.0, 36.0),
        (-62.0, 0.0, 16.0, 36.0),
    ]
    .into_iter()
    .enumerate()
    {
        w.spawn((
            Transform::from_xyz(x, hill(x, z), z),
            GrassMeadow {
                half_extents: [hx, hz],
                density: 40.0,
                height: [0.3, 0.7],
                seed: 0x4112 + i as u32,
                ..Default::default()
            },
        ));
    }
    w.spawn((
        Transform::from_xyz(-42.0, 0.0, 0.0),
        FuelPatch::firebreak(PatchShape::Rect {
            half_extents: [2.2, 110.0],
        })
        .with_priority(10),
    ));
    let dir = sun_direction(HOUR);
    w.spawn((
        DirectionalLight {
            illuminance: 110_000.0,
            ..Default::default()
        },
        Transform::default().looking_at(-dir.as_dvec3(), Vec3::Y),
    ));
    let camera = w
        .spawn((
            Camera {
                exposure: Exposure::Auto {
                    min_ev100: -3.0,
                    max_ev100: 18.0,
                    speed: 25.0,
                    compensation: 0.0,
                },
                ..Default::default()
            },
            Transform::from_xyz(-60.0, 10.0, -30.0),
        ))
        .id();

    // Let the simulation run without drawing between pictures (much faster on a software GPU).
    let mut render = Some(swap_render(&mut app, Schedule::new(Render)));
    app.update_n(4);
    let at = DVec3::new(IGNITE[0], hill(IGNITE[0], IGNITE[1]), IGNITE[1]);
    app.world.write_message(IgniteRequest {
        position: at,
        radius: 1.5,
        temperature_c: 900.0,
        source: None,
    });
    eprintln!("built the hill in {:.1} s", started.elapsed().as_secs_f32());

    let mut now = 0.0f32;
    for (view, secs) in stops {
        let t0 = std::time::Instant::now();
        if secs > now {
            app.update_n(((secs - now) * 30.0).round() as usize);
            now = secs;
        }
        let eye = DVec3::from_array(view.eye);
        let target = DVec3::from_array(view.target);
        *app.world.get_mut::<Transform>(camera).expect("camera") =
            Transform::from_translation(eye).looking_at(target, Vec3::Y);
        let empty = swap_render(&mut app, render.take().expect("render schedule"));
        app.update_n(6);
        now += 6.0 / 30.0;
        if let Some(d) = app.world.get_resource::<RenderDisabled>() {
            eprintln!("no GPU: {}", d.0);
            return;
        }
        // How far the fire has run up and down the slope from where it was lit.
        let field = app.world.resource::<engine_fire::FireField>();
        let (mut up, mut down) = (0.0f64, 0.0f64);
        for (p, c) in field.cells_near(at, 120.0) {
            use engine_fire::CellState::{Burning, Burnt, Smoldering};
            if matches!(c.state(), Burning | Smoldering | Burnt) {
                up = up.max(p.x - at.x);
                down = down.max(at.x - p.x);
            }
        }
        let path = format!("{out}/{}_{secs:.0}s.png", view.key);
        match capture_now(&mut app.world, &path) {
            Ok(_) => eprintln!(
                "{path}: {} at +{secs:.0} s: burnt {up:.0} m uphill, {down:.0} m downhill \
                 ({:.1} s)",
                view.name,
                t0.elapsed().as_secs_f32()
            ),
            Err(e) => eprintln!("{path}: {e}"),
        }
        render = Some(swap_render(&mut app, empty));
    }
}
