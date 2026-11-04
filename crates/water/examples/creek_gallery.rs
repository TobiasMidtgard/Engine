//! Renders creeks drawn without simulation (`engine_water::creeks`) over a meadow valley with a
//! network of them (`demo::creeks::Meadow`), into PNGs, without a window.
//!
//! ```text
//! cargo run -p engine_water --example creek_gallery --release -- [out_dir] [views]
//! ```
//!
//! `views` is a comma-separated subset of: creek_bank, creek_brook, creek_main, creek_mid,
//! creek_valley. `GALLERY_NO_CREEKS=1` leaves the creeks out (the dry channels, to compare);
//! `GALLERY_FRAMES=n` draws n frames a view (for `ENGINE_PROFILE=1`).

use std::sync::Arc;

use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, capture_now};
use engine_water::WaterPlugin;
use engine_water::creeks::{CreekRibbons, LocalCreeks, build_ribbons};
use engine_water::demo::creeks::{Meadow, meadow};
use engine_water::demo::materials::Surface;
use engine_water::demo::props::Part;
use engine_water::demo::scenery::spawn_prop;
use engine_world::terrain::CreekReach;
use engine_world::{Ground, GroundQuery, GroundSample, SurfaceId, WorldPlugin};

/// The meadow's carved ground as the world's ground.
struct MeadowGround(Arc<Meadow>);

impl GroundQuery for MeadowGround {
    fn sample(&self, p: DVec3) -> Option<GroundSample> {
        let h = |x: f64, z: f64| self.0.height(x as f32, z as f32);
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
        Some(self.0.height(p.x as f32, p.z as f32) as f64)
    }
}

/// A point `share` of the way down the `nth` reach of `order` (from its source), and the
/// downstream direction there.
fn on_reach(m: &Meadow, order: u8, nth: usize, share: f32) -> (DVec3, DVec3, f32) {
    let reach: &CreekReach = m
        .reaches
        .iter()
        .filter(|r| r.order == order)
        .nth(nth)
        .expect("reach");
    let k = ((reach.points.len() - 2) as f32 * share) as usize;
    let (a, b) = (reach.points[k].at, reach.points[k + 1].at);
    let dir = (b - a).with_y(0.0).normalize();
    (a, dir, reach.points[k].width_m)
}

/// (key, eye, target) for each view.
fn views(m: &Meadow) -> Vec<(&'static str, DVec3, DVec3)> {
    let ground = |p: DVec3| p.with_y(m.height(p.x as f32, p.z as f32) as f64);
    let side = |d: DVec3| DVec3::new(-d.z, 0.0, d.x);
    let mut out = Vec::new();
    // Standing on the bank of a side creek near the valley floor, looking down it over its
    // steps and pools.
    let (p, d, w) = on_reach(m, 2, 3, 0.7);
    let eye = ground(p - d * 6.0 + side(d) * (0.5 * w as f64 + 2.5)) + DVec3::Y * 1.7;
    out.push(("creek_bank", eye, p + d * 8.0));
    // Over a headwater brook tumbling down the slope.
    let (p, d, w) = on_reach(m, 1, 5, 0.5);
    let eye = ground(p + d * 7.0 + side(d) * (0.5 * w as f64 + 2.0)) + DVec3::Y * 1.7;
    out.push(("creek_brook", eye, p - d * 3.0));
    // The main stream from its bank.
    let (p, d, w) = on_reach(m, 3, 6, 0.4);
    let eye = ground(p - d * 10.0 + side(d) * (0.5 * w as f64 + 4.0)) + DVec3::Y * 2.2;
    out.push(("creek_main", eye, p + d * 14.0));
    // Over a side creek running into the main stream, from a low hill.
    let (p, d, _) = on_reach(m, 2, 4, 0.95);
    let eye = ground(p - d * 60.0 + side(d) * 35.0) + DVec3::Y * 45.0;
    out.push(("creek_mid", eye, p + d * 10.0));
    // The whole valley from the air, up it: every creek in it.
    out.push((
        "creek_valley",
        DVec3::new(520.0, 150.0, -60.0),
        DVec3::new(0.0, 10.0, 0.0),
    ));
    out
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let out = args
        .get(1)
        .cloned()
        .unwrap_or_else(|| "screenshots/creeks".into());
    let only: Option<Vec<String>> = args
        .get(2)
        .map(|s| s.split(',').map(str::to_string).collect());
    std::fs::create_dir_all(&out).expect("output directory");

    let started = std::time::Instant::now();
    let m = meadow();
    eprintln!(
        "meadow: {} reaches in {:.1} s",
        m.reaches.len(),
        started.elapsed().as_secs_f32()
    );
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
    app.world
        .insert_resource(Ground(Arc::new(MeadowGround(m.clone()))));
    let started = std::time::Instant::now();
    let mesh = m.mesh(0.75);
    eprintln!("meadow mesh in {:.1} s", started.elapsed().as_secs_f32());
    let parts = std::sync::Mutex::new(Some(vec![Part::new(mesh, Surface::Ground)]));
    app.add_systems(Startup, move |mut commands: Commands| {
        if let Some(parts) = parts.lock().ok().and_then(|mut p| p.take()) {
            spawn_prop(&mut commands, "Meadow", Transform::default(), parts);
        }
    });
    if std::env::var_os("GALLERY_NO_CREEKS").is_none() {
        app.world.spawn((
            Name::new("Meadow creeks"),
            Transform::default(),
            LocalCreeks(m.clone()),
        ));
    }
    // A late-morning sun from across the valley.
    let to_sun = Vec3::new(0.45, 0.55, -0.7).normalize();
    app.world.spawn((
        DirectionalLight {
            illuminance: 95_000.0,
            ..Default::default()
        },
        Transform::default().looking_at(-to_sun.as_dvec3(), Vec3::Y),
    ));
    let camera = app
        .world
        .spawn((Camera::default(), Transform::default()))
        .id();
    app.update_n(2);

    for (key, eye, target) in views(&m) {
        if only.as_ref().is_some_and(|o| !o.iter().any(|k| k == key)) {
            continue;
        }
        *app.world.get_mut::<Transform>(camera).expect("camera") =
            Transform::from_translation(eye).looking_at(target, Vec3::Y);
        let started = std::time::Instant::now();
        let frames = std::env::var("GALLERY_FRAMES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(8);
        app.update_n(frames);
        if let Some(d) = app.world.get_resource::<RenderDisabled>() {
            eprintln!("no GPU: {}", d.0);
            return;
        }
        // What laying them costs (the system does this when the camera has moved 48 m).
        let laid = {
            let started = std::time::Instant::now();
            let mut scratch = CreekRibbons::default();
            let providers: Vec<engine_water::creeks::Provider> = vec![(m.clone(), DVec3::ZERO)];
            build_ribbons(&providers, eye, &mut scratch);
            started.elapsed().as_secs_f64() * 1e3
        };
        let ribbons = app.world.resource::<CreekRibbons>();
        let (reaches, vertices, triangles) = (
            ribbons.reaches,
            ribbons.vertices.len(),
            ribbons.indices.len() / 3,
        );
        let path = format!("{out}/{key}.png");
        match capture_now(&mut app.world, &path) {
            Ok(_) => eprintln!(
                "{path} ({:.1} s; {reaches} creeks drawn, {vertices} vertices, {triangles} triangles, laid in {laid:.2} ms)",
                started.elapsed().as_secs_f32()
            ),
            Err(e) => eprintln!("{path}: {e}"),
        }
    }
    let stats = app.world.resource::<engine_render::RenderStats>();
    eprintln!("{stats:?}");
}
