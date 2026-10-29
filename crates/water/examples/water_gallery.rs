//! Renders the water test area from several viewpoints into PNGs, without a window.
//!
//! ```text
//! cargo run -p engine_water --example water_gallery --release -- [out_dir] [seconds] [views]
//! ```
//!
//! `seconds` of simulation run before the first picture (default 12); `views` is a
//! comma-separated subset of: overview, falls, shore, under, pond, wake, aerial, swirl, ditch,
//! cascade, crevice, ford, pools, pools_near, pools_dark.
//!
//! The dispersive ripples run on the CPU while the simulation runs without drawing, and the
//! GPU picks them up for the pictures (`ENGINE_WATER_CPU_RIPPLES=1` draws the ripple grid's
//! own waves instead, to compare). A crate stands in the stream above the ford, so the water
//! parts round it (the gallery has no physics engine: a stand-in answers the stream's
//! questions about what stands in it).

use std::sync::Arc;

use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, capture_now};
use engine_water::WaterPlugin;
use engine_water::demo::{self, LAKE_LEVEL};
use engine_water::state::SharedWater;
use engine_world::physics::{
    ColliderShape, Hit, PhysicsQueries, PhysicsQuery, QueryFilter, RigidBody,
};
use engine_world::{Ground, GroundQuery, GroundSample, SurfaceId, WorldPlugin};

/// The crate in the stream: its middle on the ground (world), half its size.
struct Crate {
    entity: Entity,
    base: DVec3,
    half: Vec3,
}

/// Answers the stream's physics questions about the crate (and nothing else).
struct CrateQueries(Vec<Crate>);

impl CrateQueries {
    fn bounds(c: &Crate) -> (DVec3, DVec3) {
        let lo = c.base - DVec3::new(c.half.x as f64, 0.0, c.half.z as f64);
        let hi = c.base + DVec3::new(c.half.x as f64, 2.0 * c.half.y as f64, c.half.z as f64);
        (lo, hi)
    }
}

impl PhysicsQuery for CrateQueries {
    fn raycast(&self, origin: DVec3, dir: Vec3, max: f32, filter: &QueryFilter) -> Option<Hit> {
        self.0
            .iter()
            .filter(|c| !filter.exclude.contains(&c.entity))
            .filter_map(|c| {
                let (lo, hi) = Self::bounds(c);
                if origin.x < lo.x || origin.x > hi.x || origin.z < lo.z || origin.z > hi.z {
                    return None;
                }
                // Straight up or down only (all the stream asks).
                let inside = origin.y >= lo.y && origin.y <= hi.y;
                let d = if inside {
                    0.0
                } else if dir.y < 0.0 && origin.y > hi.y {
                    origin.y - hi.y
                } else if dir.y > 0.0 && origin.y < lo.y {
                    lo.y - origin.y
                } else {
                    return None;
                };
                (d as f32 <= max).then(|| Hit {
                    entity: c.entity,
                    point: origin + dir.as_dvec3() * d,
                    normal: -dir,
                    distance: d as f32,
                    surface: SurfaceId::default(),
                })
            })
            .min_by(|a, b| a.distance.total_cmp(&b.distance))
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
        position: DVec3,
        _: Quat,
        _: &QueryFilter,
        out: &mut Vec<Entity>,
    ) {
        // The patch's turned box, taken as the sphere round it.
        let reach = match shape {
            ColliderShape::Cuboid { half_extents } => half_extents.length() as f64,
            _ => 0.0,
        };
        for c in &self.0 {
            let (lo, hi) = Self::bounds(c);
            let near = position.clamp(lo, hi);
            if near.distance(position) <= reach {
                out.push(c.entity);
            }
        }
    }
    fn closest_point(&self, _: DVec3, _: f32, _: &QueryFilter) -> Option<Hit> {
        None
    }
}

/// The test area's hill as the world's ground, as a world's terrain carries it in the
/// showcase (`demo::area_height`): what wets, shelters and beds on it sees the real shape.
struct AreaGround;

impl GroundQuery for AreaGround {
    fn sample(&self, p: DVec3) -> Option<GroundSample> {
        let h = |x: f64, z: f64| demo::area_height(x as f32, z as f32);
        let (x, z) = (p.x, p.z);
        let normal = Vec3::new(
            h(x - 0.25, z) - h(x + 0.25, z),
            0.5,
            h(x, z - 0.25) - h(x, z + 0.25),
        )
        .normalize();
        Some(GroundSample {
            height: h(x, z) as f64,
            normal,
            surface: SurfaceId(3),
        })
    }

    fn height(&self, p: DVec3) -> Option<f64> {
        Some(demo::area_height(p.x as f32, p.z as f32) as f64)
    }
}

/// (key, eye, target), local to the test area's center.
const VIEWS: &[(&str, [f64; 3], [f64; 3])] = &[
    ("overview", [62.0, 18.0, 66.0], [0.0, 4.0, 0.0]),
    ("falls", [34.5, 3.6, 33.0], [27.0, 4.2, 27.4]),
    ("shore", [-1.5, 7.3, 30.5], [-2.0, 5.0, 0.0]),
    ("under", [-2.0, 3.6, 7.0], [5.0, 4.6, 17.5]),
    ("pond", [56.0, 2.0, 38.0], [46.7, 0.3, 46.7]),
    ("wake", [-3.0, 11.0, 26.0], [-5.0, 5.5, -3.0]),
    ("aerial", [0.0, 95.0, 70.0], [10.0, 0.0, 8.0]),
    // Placed over the motorboat when it is found.
    ("swirl", [-5.0, 24.0, 19.0], [-5.0, 5.5, -4.0]),
    // Low behind the motorboat, down its wake (placed when it is found).
    ("kelvin", [-3.0, 9.0, 26.0], [-5.0, 5.5, -3.0]),
    ("ditch", [52.0, 30.0, 52.0], [51.0, 0.0, 50.0]),
    // Across the cove at the foot of the crag, and up close under its crevice.
    ("cascade", [-8.2, 9.0, -15.3], [-25.2, 13.0, -25.2]),
    ("crevice", [-14.9, 8.0, -18.5], [-23.0, 10.5, -23.0]),
    // The wader fording the stream above the falls.
    ("ford", [19.5, 9.0, 27.0], [24.5, 5.3, 24.0]),
    // The row of pools on the rim, one per water color, and its clearest and darkest ends.
    ("pools", [36.8, 20.0, -40.9], [21.0, 5.5, -24.0]),
    ("pools_near", [-5.6, 10.0, -39.6], [18.0, 5.0, -31.2]),
    ("pools_dark", [40.0, 10.0, 1.4], [29.1, 5.0, -21.2]),
    // The tarn as a mirror: from the end of the jetty across to the crag, low over the water
    // from the far shore, and the crag's foot from the cove.
    ("jetty", [4.6, 7.4, 18.0], [-14.0, 8.0, -20.0]),
    ("grazing", [-27.0, 6.4, 12.0], [18.0, 7.0, -6.0]),
];

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let out = args
        .get(1)
        .cloned()
        .unwrap_or_else(|| "screenshots/water".into());
    let seconds: f32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(12.0);
    let only: Option<Vec<String>> = args
        .get(3)
        .map(|s| s.split(',').map(str::to_string).collect());
    std::fs::create_dir_all(&out).expect("output directory");

    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = 1280;
        w.height = 720;
    }
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 30.0,
        });
    app.add_plugins((RenderPlugin, WorldPlugin, WaterPlugin));
    app.world.insert_resource(Ground(Arc::new(AreaGround)));
    app.world
        .resource_mut::<engine_water::WaterSettings>()
        .cpu_surface = true;

    let w = &mut app.world;
    w.spawn((
        Transform::default(),
        MeshRenderer::new(
            Shape::Plane {
                size: [2000.0, 2000.0],
                subdivisions: 1,
            },
            StandardMaterial::hex(0x55743a).with_roughness(0.95),
        ),
    ));
    // Late-afternoon sun, low over the far side of the lake from the main viewpoint.
    let to_sun = Vec3::new(-0.55, 0.36, -0.75).normalize();
    let sun = w
        .spawn((
            DirectionalLight {
                illuminance: 95_000.0,
                ..Default::default()
            },
            Transform::default().looking_at(-to_sun.as_dvec3(), Vec3::Y),
        ))
        .id();
    // Over the tarn while the simulation runs, so the ripple grid (which follows the camera)
    // covers all of it; at the one view asked for, so the water there is the live part (a
    // stream's wakes, say).
    let single = only
        .as_ref()
        .filter(|o| o.len() == 1)
        .and_then(|o| VIEWS.iter().find(|(k, ..)| *k == o[0].as_str()))
        .filter(|(k, ..)| !matches!(*k, "swirl" | "kelvin"));
    let sim_from = single.map_or(Transform::from_xyz(-3.0, 30.0, 0.0), |(_, eye, target)| {
        Transform::from_translation(DVec3::from_array(*eye))
            .looking_at(DVec3::from_array(*target), Vec3::Y)
    });
    let camera = w.spawn((Camera::default(), sim_from)).id();
    app.add_systems(Startup, |mut commands: Commands| {
        demo::spawn_demo(&mut commands, DVec3::ZERO);
    });

    // The crate, once the valley's ground is in: a meter-wide box standing in the stream a few
    // meters above the ford (`GALLERY_NO_CRATE=1` leaves it out, to compare).
    app.update_n(3);
    if std::env::var_os("GALLERY_NO_CRATE").is_none() {
        let at = demo::outlet_point(31.0);
        // On the stream's bed.
        let water = app.world.resource::<SharedWater>().clone();
        let base_y = water
            .0
            .read()
            .bed_height(at + DVec3::Y * 20.0)
            .unwrap_or(at.y);
        eprintln!("crate at {at:?}, bed {base_y:.2}");
        let half = Vec3::new(0.5, 0.45, 0.5);
        let base = DVec3::new(at.x, base_y, at.z);
        let entity = app
            .world
            .spawn((
                Name::new("Crate in the stream"),
                Transform::from_translation(base + DVec3::Y * half.y as f64),
                MeshRenderer::new(
                    Shape::Cuboid {
                        size: (half * 2.0).to_array(),
                    },
                    StandardMaterial::hex(0x8a6a44).with_roughness(0.8),
                ),
                // Moved by gameplay, not the valley's scenery: the stream goes round it.
                RigidBody::dynamic(),
            ))
            .id();
        app.world
            .insert_resource(PhysicsQueries(Arc::new(CrateQueries(vec![Crate {
                entity,
                base,
                half,
            }]))));
    }

    // Let the simulation run without drawing (much faster on a software GPU).
    let render = app
        .world
        .resource_mut::<Schedules>()
        .remove(Render)
        .expect("render schedule");
    app.world
        .resource_mut::<Schedules>()
        .insert(Schedule::new(Render));
    let frames = (seconds * 30.0) as usize;
    let started = std::time::Instant::now();
    app.update_n(frames.max(2) - 1);
    let boat_at = |app: &mut App| {
        let mut q = app.world.query::<(&Name, &GlobalTransform)>();
        q.iter(&app.world)
            .find(|(n, _)| n.as_str() == "Motorboat")
            .map(|(_, g)| g.translation())
    };
    let before = boat_at(&mut app);
    app.update_n(1);
    if let (Some(a), Some(b)) = (before, boat_at(&mut app)) {
        eprintln!("motorboat at {:.1} m/s", (b - a).length() * 30.0);
    }
    eprintln!(
        "simulated {seconds} s in {:.1} s",
        started.elapsed().as_secs_f32()
    );
    {
        let water = app.world.resource::<SharedWater>().clone();
        let state = water.0.read();
        let link = &state.ripples.surface;
        let peak = link
            .heights()
            .map_or(0.0, |h| h.h.iter().fold(0.0f32, |m, v| m.max(v.abs())));
        eprintln!(
            "surface: {:?}, highest ripple {peak:.4} m (grid's own {:.4} m)",
            link.runner(),
            state.ripples.h.iter().fold(0.0f32, |m, v| m.max(v.abs()))
        );
        // The surfaces side by side, seen from above (the grid's own, the dispersive fine
        // level over it, and its coarse level four times larger when it runs): what the water
        // itself feels, without the light. Red up, blue down, full colour at 5 cm; last, the
        // fine level's steepness.
        if let Some(h) = link.heights() {
            let n = state.ripples.n;
            let scale = 0.05;
            let coarse = link.coarse();
            // (And the fine level's slope, full colour at 0.2: what the light sees.)
            let slope: Vec<f32> = (0..n * n)
                .map(|k| {
                    let (i, j) = (k % n, k / n);
                    let at = |ii: usize, jj: usize| h.h[jj.min(n - 1) * n + ii.min(n - 1)];
                    let gx = at(i + 1, j) - at(i.saturating_sub(1), j);
                    let gz = at(i, j + 1) - at(i, j.saturating_sub(1));
                    (gx * gx + gz * gz).sqrt() / (2.0 * state.ripples.cell) * scale / 0.2
                })
                .collect();
            let mut panels = vec![&state.ripples.h, &h.h];
            if let Some(c) = &coarse {
                panels.push(&c.h);
            }
            panels.push(&slope);
            let mut img = image::RgbImage::new(((n + 8) * panels.len() - 8) as u32, n as u32);
            for (side, buf) in panels.into_iter().enumerate() {
                for j in 0..n {
                    for i in 0..n {
                        let v = (buf[j * n + i] / scale).clamp(-1.0, 1.0);
                        let c = if v > 0.0 {
                            [255, (255.0 * (1.0 - v)) as u8, (255.0 * (1.0 - v)) as u8]
                        } else {
                            [(255.0 * (1.0 + v)) as u8, (255.0 * (1.0 + v)) as u8, 255]
                        };
                        img.put_pixel((side * (n + 8) + i) as u32, j as u32, image::Rgb(c));
                    }
                }
            }
            let path = format!("{out}/heights.png");
            img.save(&path).expect("heights image");
            eprintln!("{path}");
        }
    }
    // The wind's reach over the tarn (black: sheltered, white: open), north up.
    {
        let mask = app.world.resource::<engine_water::shelter::ShelterMask>();
        if mask.exposure.len() == engine_water::shelter::MASK_N.pow(2) {
            let n = engine_water::shelter::MASK_N as u32;
            let img = image::GrayImage::from_fn(n, n, |i, j| {
                image::Luma([(mask.exposure[(j * n + i) as usize] * 255.0) as u8])
            });
            let path = format!("{out}/shelter.png");
            img.save(&path).expect("shelter image");
            eprintln!("{path} (wind toward {:?})", mask.wind_dir);
        }
    }
    app.world.resource_mut::<Schedules>().insert(render);

    for (key, eye, target) in VIEWS {
        if only.as_ref().is_some_and(|o| !o.iter().any(|k| k == key)) {
            continue;
        }
        let (mut eye, mut target) = (DVec3::from_array(*eye), DVec3::from_array(*target));
        // The swirl view looks down on the motorboat.
        if *key == "swirl" {
            let mut q = app.world.query::<(&Name, &GlobalTransform)>();
            if let Some((_, g)) = q.iter(&app.world).find(|(n, _)| n.as_str() == "Motorboat") {
                let boat = g.translation();
                eye = boat + DVec3::new(0.0, 22.0, 14.0);
                target = boat;
            }
        }
        if *key == "kelvin" {
            let mut q = app.world.query::<(&Name, &GlobalTransform)>();
            if let Some((_, g)) = q.iter(&app.world).find(|(n, _)| n.as_str() == "Motorboat") {
                let boat = g.translation();
                let fwd = (g.rotation() * Vec3::X)
                    .with_y(0.0)
                    .normalize_or_zero()
                    .as_dvec3();
                let side = DVec3::new(-fwd.z, 0.0, fwd.x);
                eye = boat - fwd * 13.0 + side * 6.0 + DVec3::Y * 4.5;
                target = boat - fwd * 2.0;
            }
        }
        *app.world.get_mut::<Transform>(camera).expect("camera") =
            Transform::from_translation(eye).looking_at(target, Vec3::Y);
        // The crag faces away from the afternoon sun: show it in the morning.
        let light = if matches!(*key, "cascade" | "crevice") {
            Vec3::new(0.55, 0.5, 0.35).normalize()
        } else {
            to_sun
        };
        *app.world.get_mut::<Transform>(sun).expect("sun") =
            Transform::default().looking_at(-light.as_dvec3(), Vec3::Y);
        // A few frames for exposure, the environment map and the ripples to follow the camera.
        let started = std::time::Instant::now();
        // (`GALLERY_FRAMES=n`: draw n frames here instead, for `ENGINE_PROFILE=1`.)
        let frames = std::env::var("GALLERY_FRAMES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(6);
        app.update_n(frames);
        if let Some(d) = app.world.get_resource::<RenderDisabled>() {
            eprintln!("no GPU: {}", d.0);
            return;
        }
        let path = format!("{out}/{key}.png");
        match capture_now(&mut app.world, &path) {
            Ok(_) => eprintln!(
                "{path} ({:.1} s; water level {LAKE_LEVEL} m)",
                started.elapsed().as_secs_f32()
            ),
            Err(e) => eprintln!("{path}: {e}"),
        }
    }
    {
        // What the renderer drew from (the GPU picks the surface up from the CPU).
        let water = app.world.resource::<SharedWater>().clone();
        let state = water.0.read();
        let link = &state.ripples.surface;
        let peak = link
            .heights()
            .map_or(0.0, |h| h.h.iter().fold(0.0f32, |m, v| m.max(v.abs())));
        eprintln!(
            "surface after the views: {:?}, highest ripple {peak:.4} m",
            link.runner()
        );
    }
    let stats = app.world.resource::<engine_render::RenderStats>();
    eprintln!("{stats:?}");
}
