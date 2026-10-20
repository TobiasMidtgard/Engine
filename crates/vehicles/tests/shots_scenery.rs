//! The demo scenery: build budgets for each zone (time and triangles), and offscreen shots
//! of the race track, airfield and spaceport from overview, driver / pilot eye height and
//! close up, with the demo vehicles running. Needs a GPU or a software Vulkan driver
//! (lavapipe) for the shots; they pass with a note when neither is available.
//!
//! The shots render only with `$VEHICLE_SHOTS` set to a folder for the PNGs (the budget
//! always runs); `$SCENERY_SHOTS_ONLY` (comma-separated name parts) limits which are taken.

mod support;

use engine_assets::Assets;
use engine_core::math::{DVec3, Vec3};
use engine_core::prelude::*;
use engine_render::prelude::*;
use engine_render::{Image, Mesh, RenderDisabled, capture_now};
use engine_vehicles::models::scenery::{self, BuildStats};
use support::shots::gallery;
use support::*;

/// Generation must stay under a second per zone (dev profile) and a million triangles.
#[test]
fn scenery_build_budget() {
    if let Err(e) = budget() {
        panic!("{e}");
    }
}

/// The build budget, measured once per test run before any shot test builds its scene (they
/// all wait for it), so other tests baking textures and rendering in parallel do not inflate
/// the timings.
fn budget() -> &'static Result<(), String> {
    static RESULT: std::sync::OnceLock<Result<(), String>> = std::sync::OnceLock::new();
    RESULT.get_or_init(measure_budget)
}

/// Builds one zone's scenery into a world.
type ZoneBuild = Box<dyn Fn(&mut World) -> Option<BuildStats>>;

fn measure_budget() -> Result<(), String> {
    let mut world = World::new();
    world.insert_resource(Assets::<Mesh>::default());
    world.insert_resource(Assets::<Image>::default());
    world.insert_resource(Assets::<StandardMaterial>::default());
    let zones: [(&str, ZoneBuild); 3] = [
        (
            "track",
            Box::new(|w| scenery::track::build(w, DVec3::new(0.0, 0.0, -500.0))),
        ),
        (
            "airfield",
            Box::new(|w| {
                scenery::airfield::build(
                    w,
                    DVec3::new(780.0, 0.0, -750.0),
                    DVec3::new(780.0, 0.0, 450.0),
                )
            }),
        ),
        (
            "spaceport",
            Box::new(|w| scenery::spaceport::build(w, DVec3::new(470.0, 0.4, 280.0), 30.0)),
        ),
    ];
    for (name, build) in &zones {
        let stats = build(&mut world).ok_or("renderer assets missing")?;
        eprintln!(
            "scenery {name}: {:.0} ms, {} triangles, {} entities",
            stats.seconds * 1000.0,
            stats.triangles,
            stats.entities
        );
        // The heaviest meshes of the zone.
        let meshes = world.resource::<Assets<Mesh>>();
        let mut heavy: Vec<(String, usize)> = meshes
            .keys()
            .filter(|(k, _)| k.contains(&format!("/{name}@")))
            .filter_map(|(k, h)| {
                let tris = meshes.get(h)?.triangle_count();
                Some((k.rsplit('/').next().unwrap_or(k).to_string(), tris))
            })
            .collect();
        heavy.sort_by_key(|h| std::cmp::Reverse(h.1));
        let list: Vec<String> = heavy
            .iter()
            .take(8)
            .map(|(k, t)| format!("{k} {t}"))
            .collect();
        eprintln!("  heaviest: {}", list.join(", "));
        if stats.seconds > 1.0 {
            return Err(format!("{name} took {:.2} s", stats.seconds));
        }
        if stats.triangles > 1_000_000 {
            return Err(format!("{name}: {} triangles", stats.triangles));
        }
    }
    let bake = scenery::finish_textures(&mut world);
    eprintln!("scenery textures: full-size bakes finished {bake:.1} s after the last zone");
    // A second build of a zone reuses every texture and material.
    let again = scenery::track::build(&mut world, DVec3::new(0.0, 0.0, 500.0))
        .ok_or("renderer assets missing")?;
    eprintln!("scenery track again: {:.0} ms", again.seconds * 1000.0);
    if again.seconds > 1.0 {
        return Err(format!("track rebuild took {:.2} s", again.seconds));
    }
    Ok(())
}

/// One frame to save: when (seconds of simulation since the scene started) and from where.
struct Shot {
    name: &'static str,
    at: f64,
    camera: Transform,
}

fn shot(name: &'static str, at: f64, eye: [f64; 3], target: [f64; 3]) -> Shot {
    Shot {
        name,
        at,
        camera: Transform::from_translation(DVec3::from_array(eye))
            .looking_at(DVec3::from_array(target), Vec3::Y),
    }
}

/// Render several shots of one scene: the scene simulates with the renderer held back, and
/// at each shot's time a few frames render from its camera and one is saved.
fn shoot_scene(scene: impl FnOnce(&mut App), mut shots: Vec<Shot>) {
    // Let the budget measurement finish first (its result is the budget test's to report).
    let _ = budget();
    if !gallery(shots.first().map_or("scenery", |s| s.name)) {
        return;
    }
    let mut app = vehicle_app();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = 1280;
        w.height = 720;
    }
    app.add_plugins(RenderPlugin);
    calm(&mut app);
    app.insert_resource(RenderDisabled("warming up".into()));
    spawn_with(&mut app, |c| {
        c.spawn((
            DirectionalLight::default(),
            Transform::default().looking_at(DVec3::new(-0.45, -0.55, -0.7), Vec3::Y),
        ))
        .id()
    });
    scene(&mut app);
    let waited = scenery::finish_textures(&mut app.world);
    eprintln!("textures ready after {waited:.1} s");
    // `SCENERY_SHOTS_ONLY=name,name` keeps only the shots whose names contain one of them.
    if let Ok(only) = std::env::var("SCENERY_SHOTS_ONLY") {
        shots.retain(|s| only.split(',').any(|k| !k.is_empty() && s.name.contains(k)));
    }
    shots.sort_by(|a, b| a.at.total_cmp(&b.at));
    let camera = spawn_with(&mut app, |c| {
        c.spawn((Camera::perspective(45.0), Transform::default()))
            .id()
    });
    let dir = std::env::var("VEHICLE_SHOTS")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")));
    std::fs::create_dir_all(&dir).expect("screenshot dir");
    let mut now = 0.0;
    for s in shots {
        if s.at > now {
            run_for(&mut app, s.at - now);
            now = s.at;
        }
        *app.world.get_mut::<Transform>(camera).expect("camera") = s.camera;
        app.world.remove_resource::<RenderDisabled>();
        for _ in 0..10 {
            app.update();
        }
        now += 10.0 * DT;
        if let Some(d) = app.world.get_resource::<RenderDisabled>() {
            eprintln!("skipping {}: no GPU ({})", s.name, d.0);
            return;
        }
        let path = dir.join(format!("{}.png", s.name));
        capture_now(&mut app.world, &path).expect("screenshot");
        eprintln!("{} -> {}", s.name, path.display());
        app.insert_resource(RenderDisabled("between shots".into()));
    }
}

#[test]
fn scenery_track_shots() {
    shoot_scene(
        |app| {
            scenery::grass_ground(&mut app.world, DVec3::new(0.0, -0.01, 0.0), 450.0);
            spawn_with(app, |c| {
                engine_vehicles::demo::spawn_track(c, DVec3::ZERO);
                c.spawn_empty().id()
            });
        },
        vec![
            shot(
                "scenery_track_overview",
                2.0,
                [60.0, 60.0, 160.0],
                [0.0, 0.0, 0.0],
            ),
            shot(
                "scenery_track_overview_high",
                2.0,
                [-150.0, 140.0, -170.0],
                [10.0, 0.0, 0.0],
            ),
            shot(
                "scenery_track_grid",
                2.5,
                [-48.0, 1.15, -82.7],
                [-10.0, 0.6, -80.0],
            ),
            shot(
                "scenery_track_start",
                2.5,
                [8.0, 3.0, -84.0],
                [-6.0, 4.0, -108.0],
            ),
            shot(
                "scenery_track_pits",
                2.5,
                [10.0, 2.5, -78.0],
                [-14.0, 3.0, -58.0],
            ),
            shot(
                "scenery_track_corner",
                2.5,
                [118.0, 1.2, -50.0],
                [140.0, 0.6, 5.0],
            ),
            shot(
                "scenery_track_curbs",
                2.5,
                [131.0, 2.4, -35.0],
                [139.0, 0.0, -18.0],
            ),
            shot(
                "scenery_track_barriers",
                3.0,
                [150.0, 2.0, -10.0],
                [158.0, 0.6, 18.0],
            ),
            shot(
                "scenery_track_kicker",
                3.0,
                [24.0, 1.7, 83.5],
                [0.0, 0.6, 80.0],
            ),
            shot(
                "scenery_track_drift",
                3.0,
                [-45.0, 9.0, -5.0],
                [-70.0, 0.0, -20.0],
            ),
            shot(
                "scenery_track_paddock",
                1.0,
                [-22.0, 2.6, -3.0],
                [6.0, 2.4, -24.0],
            ),
            shot(
                "scenery_track_pitback",
                1.0,
                [-30.0, 2.0, -33.0],
                [-5.0, 3.5, -41.0],
            ),
        ],
    );
}

#[test]
fn scenery_airfield_shots() {
    shoot_scene(
        |app| {
            scenery::grass_ground(&mut app.world, DVec3::new(0.0, -0.01, 0.0), 900.0);
            spawn_with(app, |c| {
                engine_vehicles::demo::spawn_airfield(
                    c,
                    DVec3::new(0.0, 0.0, -600.0),
                    DVec3::new(0.0, 0.0, 600.0),
                );
                c.spawn_empty().id()
            });
        },
        vec![
            shot(
                "scenery_airfield_threshold",
                0.5,
                [-7.0, 3.2, -628.0],
                [0.0, 1.0, -500.0],
            ),
            shot(
                "scenery_airfield_paint_closeup",
                0.5,
                [-8.5, 1.6, -599.0],
                [-3.0, 0.0, -587.0],
            ),
            shot(
                "scenery_airfield_threshold_top",
                0.5,
                [0.0, 40.0, -575.0],
                [0.0, 0.0, -574.9],
            ),
            shot(
                "scenery_airfield_markings",
                0.5,
                [36.0, 45.0, -660.0],
                [0.0, 0.0, -560.0],
            ),
            shot(
                "scenery_airfield_overview",
                1.0,
                [-160.0, 25.0, -70.0],
                [-40.0, 0.0, 0.0],
            ),
            shot(
                "scenery_airfield_overview_high",
                1.0,
                [-140.0, 110.0, -190.0],
                [0.0, 0.0, 10.0],
            ),
            shot(
                "scenery_airfield_hangar",
                1.5,
                [22.0, 5.0, 34.0],
                [70.0, 6.0, 6.0],
            ),
            shot(
                "scenery_airfield_tower",
                1.5,
                [-38.0, 6.0, 8.0],
                [-79.0, 12.0, -24.0],
            ),
            shot(
                "scenery_airfield_papi",
                1.5,
                [30.0, 1.5, -324.0],
                [38.0, 0.9, -300.0],
            ),
            shot(
                "scenery_airfield_signs",
                1.5,
                [-50.0, 1.7, -6.0],
                [-40.5, 1.0, -11.5],
            ),
            shot(
                "scenery_airfield_fire",
                1.5,
                [40.0, 3.0, -44.0],
                [66.0, 4.5, -28.0],
            ),
            shot(
                "scenery_airfield_terminal",
                1.5,
                [-57.0, 2.5, 19.0],
                [-77.0, 3.5, 3.0],
            ),
        ],
    );
}

#[test]
fn scenery_spaceport_shots() {
    shoot_scene(
        |app| {
            scenery::grass_ground(&mut app.world, DVec3::new(0.0, -0.01, 0.0), 450.0);
            spawn_with(app, |c| {
                engine_vehicles::demo::spawn_spaceport(c, DVec3::new(0.0, 0.4, 0.0), 30.0);
                c.spawn_empty().id()
            });
        },
        vec![
            shot(
                "scenery_spaceport_overview",
                2.0,
                [-95.0, 70.0, -115.0],
                [0.0, 4.0, 0.0],
            ),
            shot(
                "scenery_spaceport_view",
                2.0,
                [-70.0, 10.0, -30.0],
                [20.0, 30.0, 20.0],
            ),
            shot(
                "scenery_spaceport_water",
                2.0,
                [12.0, 4.0, -48.0],
                [-18.0, 28.0, -82.0],
            ),
            shot(
                "scenery_spaceport_grate",
                2.5,
                [9.0, 7.0, 12.0],
                [0.0, 0.4, 0.0],
            ),
            shot(
                "scenery_spaceport_hangars",
                3.0,
                [-30.0, 3.0, -2.0],
                [-80.0, 7.0, -8.0],
            ),
            shot(
                "scenery_spaceport_tanks",
                3.0,
                [-8.0, 6.0, 40.0],
                [-36.0, 5.0, 76.0],
            ),
            shot(
                "scenery_spaceport_tower",
                3.0,
                [4.0, 7.0, -24.0],
                [40.0, 17.0, 8.0],
            ),
            shot(
                "scenery_spaceport_pad",
                3.5,
                [-27.0, 2.0, 25.0],
                [0.0, 8.0, 0.0],
            ),
        ],
    );
}
