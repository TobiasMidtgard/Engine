//! Close-up stills of every stretch of the footstep trail, for judging the ground and props:
//! one view from eye height (1.7 m) along the path and one low three-quarter view from the
//! side, plus a wide view of the whole loop. Same scene as `trail_recording` (sun, lawn
//! and trees). Views of where one ground meets the next are named after both
//! (`mud-snow`, `snow-ice`, ...) and rendered only when asked for with `--only`.
//!
//! ```text
//! cargo run -p engine_audio --release --example trail_stills -- [out_dir]
//!     [--only grass,mud,mud-snow,wide] [--size 1280x720] [--walkers] [--panels]
//!     [--wide eye_x,eye_y,eye_z,at_x,at_y,at_z] [--sun x,y,z] [--ev ev100]
//!     [--follow dog --on mud[+meters] --secs 2 --frames 6 --every 0.25]
//! cargo run -p engine_audio --example trail_stills -- --measure
//! ```
//!
//! Writes `ground_<surface>.png` (the two views stacked) and `ground_wide.png`. `--panels`
//! also keeps each view as its own file. `--measure` builds the trail twice (a cold and a
//! warm spawn) and prints how long `spawn_demo` and the first frame take, and how many
//! entities and triangles the trail adds; it renders nothing. `--sun` sets the direction the
//! sunlight travels (default a low afternoon sun) and `--ev` a fixed exposure for every view.
//! `--follow` (hiker, runner, dog or horse) instead puts that walker `meters` into the stretch
//! named by `--on`, lets the clock run `--secs`, and films it from behind as the showcase's
//! follow camera does (`follow_<walker>_<stretch>_<k>.png`, `--frames` of them `--every`
//! seconds apart), sound rings and all, and records what the camera hears from the start
//! (`follow_<walker>_<stretch>.wav`): `--secs 0 --frames 360 --every 0.0333` is a 12 s clip.
//! Headless rendering needs a GPU or lavapipe (`mesa-vulkan-drivers`).

use std::path::{Path, PathBuf};
use std::time::Instant;

use engine_audio::demo::{
    STRETCHES, SoundRings, TrailWalker, spawn_backdrop, spawn_demo, stretch_start, trail_point,
};
use engine_audio::{Audio, AudioOutput, AudioPlugin, AudioSettings};
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_render::{
    Atmosphere, Camera, DirectionalLight, Exposure, Mesh, RenderPlugin, ResolvedMesh, Screenshots,
};
use engine_world::WorldPlugin;

struct Options {
    out: PathBuf,
    only: Option<Vec<String>>,
    size: (u32, u32),
    walkers: bool,
    panels: bool,
    measure: bool,
    /// Wide view: eye and target, meters from the trail's origin.
    wide: [f64; 6],
    /// Direction the sunlight travels.
    sun: DVec3,
    /// Exposure for every view (otherwise one per stretch).
    ev100: Option<f32>,
    /// `--follow`: the walker, the stretch and meters into it, seconds to run first, and
    /// frames to film how many seconds apart.
    follow: Option<String>,
    on: (String, f64),
    secs: f32,
    frames: u32,
    every: f32,
}

fn options() -> Options {
    let mut o = Options {
        out: PathBuf::from("trail_stills"),
        only: None,
        size: (1280, 720),
        walkers: false,
        panels: false,
        measure: false,
        wide: [-2.0, 22.0, -29.0, 0.0, 0.0, 1.5],
        sun: DVec3::new(-0.55, -0.42, -0.62),
        ev100: None,
        follow: None,
        on: ("mud".into(), 2.0),
        secs: 2.0,
        frames: 6,
        every: 0.25,
    };
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < args.len() {
        let next = args.get(i + 1).cloned().unwrap_or_default();
        match args[i].as_str() {
            "--only" => {
                o.only = Some(next.split(',').map(str::to_string).collect());
                i += 1;
            }
            "--size" => {
                let (w, h) = next.split_once('x').expect("--size takes WxH");
                o.size = (w.parse().expect("width"), h.parse().expect("height"));
                i += 1;
            }
            "--walkers" => o.walkers = true,
            "--panels" => o.panels = true,
            "--measure" => o.measure = true,
            "--wide" => {
                let v: Vec<f64> = next
                    .split(',')
                    .map(|x| x.parse().expect("number"))
                    .collect();
                o.wide.copy_from_slice(&v);
                i += 1;
            }
            "--sun" => {
                let v: Vec<f64> = next
                    .split(',')
                    .map(|x| x.parse().expect("number"))
                    .collect();
                o.sun = DVec3::new(v[0], v[1], v[2]);
                i += 1;
            }
            "--follow" => {
                o.follow = Some(next);
                i += 1;
            }
            "--on" => {
                let (name, m) = next.split_once('+').unwrap_or((&next, "2"));
                o.on = (
                    name.to_string(),
                    m.parse().expect("--on takes stretch[+meters]"),
                );
                i += 1;
            }
            "--secs" => {
                o.secs = next.parse().expect("--secs takes seconds");
                i += 1;
            }
            "--frames" => {
                o.frames = next.parse().expect("--frames takes a count");
                i += 1;
            }
            "--every" => {
                o.every = next.parse().expect("--every takes seconds");
                i += 1;
            }
            "--ev" => {
                o.ev100 = Some(next.parse().expect("--ev takes an EV100"));
                i += 1;
            }
            a if !a.starts_with("--") => o.out = PathBuf::from(a),
            a => panic!("unknown option {a}"),
        }
        i += 1;
    }
    o
}

/// A camera: eye, target and vertical field of view in degrees.
type View = (DVec3, DVec3, f32);

fn main() {
    engine_core::log::init("warn,wgpu=error,naga=warn");
    let o = options();
    if o.measure {
        measure();
        return;
    }
    std::fs::create_dir_all(&o.out).expect("create output folder");

    let mut app = scene_app(o.size, true);
    app.update();
    let mut suns = app
        .world
        .query_filtered::<&mut Transform, With<DirectionalLight>>();
    for mut t in suns.iter_mut(&mut app.world) {
        *t = Transform::default().looking_at(o.sun, Vec3::Y);
    }
    if let Some(who) = &o.follow {
        film_walker(&mut app, &o, who, &o.on.0, o.on.1);
        return;
    }
    if !o.walkers {
        let walkers: Vec<Entity> = app
            .world
            .query_filtered::<Entity, With<TrailWalker>>()
            .iter(&app.world)
            .collect();
        for e in walkers {
            app.world.despawn(e);
        }
        // No walkers, no sounds: the rings of the first frame's steps would otherwise stay
        // lit in every view (the clock barely moves between stills).
        app.world.resource_mut::<SoundRings>().enabled = false;
    }
    // Let shaders compile and the first uploads happen.
    app.update();
    if !o.walkers {
        let rings: Vec<Entity> = app
            .world
            .query::<(Entity, &Name)>()
            .iter(&app.world)
            .filter(|(_, n)| n.as_str() == "Sound ring")
            .map(|(e, _)| e)
            .collect();
        for e in rings {
            app.world.despawn(e);
        }
    }

    // Name, views, and whether it is only rendered when asked for by name.
    let mut shots: Vec<(String, Vec<View>, bool)> = Vec::new();
    for &(name, len) in STRETCHES {
        let s0 = stretch_start(name).unwrap_or(0.0);
        let mid = s0 + len * 0.5;
        let (p, d) = trail_point(mid);
        let right = Vec3::new(-d.z, 0.0, d.x).as_dvec3();
        // Outside of the loop (the underpass is open on that side).
        let outward = if p.dot(right) > 0.0 { right } else { -right };
        let (back, _) = trail_point(mid - 2.4);
        let (ahead, _) = trail_point(mid + 1.6);
        let eye = (
            back + outward * 0.25 + DVec3::new(0.0, 1.7, 0.0),
            ahead + DVec3::new(0.0, 0.05, 0.0),
            60.0,
        );
        let (side, _) = trail_point(mid - 1.3);
        let (look, _) = trail_point(mid + 0.4);
        let low = (
            side + outward * 2.3 + DVec3::new(0.0, 0.62, 0.0),
            look + DVec3::new(0.0, 0.08, 0.0),
            55.0,
        );
        shots.push((name.to_string(), vec![eye, low], false));
    }
    // Where each ground meets the next: from eye height before the boundary, and low from
    // the side across it.
    for (k, &(a, _)) in STRETCHES.iter().enumerate() {
        let b = STRETCHES[(k + 1) % STRETCHES.len()].0;
        let at = stretch_start(b).unwrap_or(0.0);
        let (p, d) = trail_point(at);
        let right = Vec3::new(-d.z, 0.0, d.x).as_dvec3();
        let outward = if p.dot(right) > 0.0 { right } else { -right };
        let (back, _) = trail_point(at - 2.6);
        let (ahead, _) = trail_point(at + 0.9);
        let eye = (
            back + outward * 0.3 + DVec3::new(0.0, 1.7, 0.0),
            ahead + DVec3::new(0.0, 0.05, 0.0),
            60.0,
        );
        let (side, _) = trail_point(at - 0.8);
        let low = (
            side + outward * 2.1 + DVec3::new(0.0, 0.6, 0.0),
            p + DVec3::new(0.0, 0.06, 0.0),
            55.0,
        );
        shots.push((format!("{a}-{b}"), vec![eye, low], true));
    }
    shots.push((
        "wide".into(),
        vec![(
            DVec3::new(o.wide[0], o.wide[1], o.wide[2]),
            DVec3::new(o.wide[3], o.wide[4], o.wide[5]),
            52.0,
        )],
        false,
    ));

    for (name, views, on_request) in shots {
        let wanted = match &o.only {
            Some(only) => only.contains(&name),
            None => !on_request,
        };
        if !wanted {
            continue;
        }
        let started = Instant::now();
        let mut files = Vec::new();
        for (k, (eye, at, fov)) in views.iter().enumerate() {
            let file = o.out.join(format!("ground_{name}_{k}.png"));
            {
                let mut q = app
                    .world
                    .query_filtered::<(&mut Transform, &mut Camera), Without<TrailWalker>>();
                for (mut t, mut cam) in q.iter_mut(&mut app.world) {
                    *t = Transform::from_translation(*eye).looking_at(*at, Vec3::Y);
                    cam.projection = engine_render::Projection::Perspective {
                        fov_y_deg: *fov,
                        near: 0.05,
                    };
                    cam.exposure = Exposure::Manual {
                        ev100: o
                            .ev100
                            .unwrap_or(if name == "concrete" { 13.6 } else { 14.6 }),
                    };
                }
            }
            // One frame to settle shadows and the sky occlusion map round the new view.
            app.update();
            app.world.resource_mut::<Screenshots>().request(&file);
            app.update();
            files.push(file);
        }
        let out = o.out.join(format!("ground_{name}.png"));
        stack(&files, &out);
        if !o.panels && files.len() > 1 {
            for f in &files {
                let _ = std::fs::remove_file(f);
            }
        } else if files.len() == 1 {
            let _ = std::fs::remove_file(&files[0]);
        }
        println!(
            "{} ({:.1} s)",
            out.display(),
            started.elapsed().as_secs_f32()
        );
    }
}

/// Put the walker whose name holds `who` `meters` into stretch `on`, run the clock, and film it
/// from behind and above (the showcase's follow camera: 6.5 and 2.3 times its size, a
/// person's being 1).
fn film_walker(app: &mut App, o: &Options, who: &str, on: &str, meters: f64) {
    let start = stretch_start(on).unwrap_or_else(|| panic!("no stretch {on}")) + meters;
    let mut q = app.world.query::<(Entity, &Name, &mut TrailWalker)>();
    let mut found = None;
    for (e, name, mut w) in q.iter_mut(&mut app.world) {
        if name.as_str().to_lowercase().contains(who) {
            w.along = start;
            found = Some(e);
        }
    }
    let walker = found.unwrap_or_else(|| panic!("no walker called {who}"));
    let size = match who {
        "dog" => 0.56 / 1.75,
        "horse" => 1.52 / 1.75,
        _ => 1.0,
    };
    let step = 1.0 / 30.0;
    let wav = o.out.join(format!("follow_{who}_{on}.wav"));
    app.world
        .resource_mut::<Audio>()
        .start_recording(&wav, o.secs + o.frames as f32 * o.every + 1.0)
        .expect("start recording");
    let mut t = 0.0;
    let mut shot = 0;
    let mut next = o.secs;
    while shot < o.frames {
        let w = app
            .world
            .get::<TrailWalker>(walker)
            .expect("walker")
            .clone();
        let (_, d) = trail_point(w.along);
        let at = app
            .world
            .get::<Transform>(walker)
            .expect("walker")
            .translation;
        let heading = d.as_dvec3();
        let eye = at - heading * (6.5 * size) + DVec3::Y * (2.3 * size).max(1.5 * size);
        let look = at + DVec3::Y * (1.1 * size);
        let mut cams = app
            .world
            .query_filtered::<(&mut Transform, &mut Camera), Without<TrailWalker>>();
        for (mut tf, mut cam) in cams.iter_mut(&mut app.world) {
            *tf = Transform::from_translation(eye).looking_at(look, Vec3::Y);
            cam.projection = engine_render::Projection::Perspective {
                fov_y_deg: 55.0,
                near: 0.05,
            };
            cam.exposure = Exposure::Manual {
                ev100: o.ev100.unwrap_or(14.6),
            };
        }
        if t + 1e-4 >= next {
            let file = o.out.join(format!("follow_{who}_{on}_{shot}.png"));
            app.world.resource_mut::<Screenshots>().request(&file);
            app.update();
            println!("{}", file.display());
            shot += 1;
            next += o.every;
        } else {
            app.update();
        }
        t += step;
    }
    let (path, secs) = app
        .world
        .resource_mut::<Audio>()
        .stop_recording()
        .expect("write the recording");
    println!("sound: {} ({secs:.1} s)", path.display());
}

/// Stack images vertically into one file.
fn stack(files: &[PathBuf], out: &Path) {
    let imgs: Vec<image::RgbaImage> = files
        .iter()
        .map(|f| image::open(f).expect("rendered still").to_rgba8())
        .collect();
    let w = imgs.iter().map(|i| i.width()).max().unwrap_or(1);
    let h: u32 = imgs.iter().map(|i| i.height()).sum();
    let mut all = image::RgbaImage::new(w, h);
    let mut y = 0;
    for i in &imgs {
        image::imageops::replace(&mut all, i, 0, y as i64);
        y += i.height();
    }
    all.save(out).expect("write still");
}

#[derive(Resource)]
struct WithDemo(bool);

fn scene_app(size: (u32, u32), demo: bool) -> App {
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = size.0;
        w.height = size.1;
    }
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 30.0,
        });
    app.insert_resource(AudioSettings {
        output: AudioOutput::Offline,
        ..Default::default()
    });
    app.add_plugins(RenderPlugin);
    app.add_plugins((WorldPlugin, AudioPlugin));
    if let Some(mut a) = app.world.get_resource_mut::<Atmosphere>() {
        a.ground_color = [0.09, 0.18, 0.04];
    }
    app.insert_resource(WithDemo(demo));
    app.add_systems(Startup, setup);
    app
}

fn setup(mut commands: Commands, demo: Res<WithDemo>) {
    let origin = DVec3::ZERO;
    commands.spawn((
        DirectionalLight::default(),
        Transform::default().looking_at(DVec3::new(-0.55, -0.42, -0.62), Vec3::Y),
    ));
    // Ground, a lawn where the trail's meadow ends, and trees placed so their shadows
    // miss the trail.
    spawn_backdrop(&mut commands, origin);
    if demo.0 {
        spawn_demo(&mut commands, origin);
    }
    commands.spawn((
        Camera::perspective(50.0),
        Transform::from_xyz(-24.0, 3.0, -14.0),
    ));
}

/// Entities and triangles drawn by mesh renderers.
fn census(world: &mut World) -> (usize, u64) {
    let entities = world.query::<Entity>().iter(world).count();
    let handles: Vec<_> = world
        .query::<&ResolvedMesh>()
        .iter(world)
        .map(|r| r.mesh)
        .collect();
    let meshes = world.resource::<engine_assets::Assets<Mesh>>();
    let tris = handles
        .iter()
        .filter_map(|h| meshes.get(*h))
        .map(|m| m.triangle_count() as u64)
        .sum();
    (entities, tris)
}

/// Time `spawn_demo` (cold, then warm in the same world) and count what it adds.
fn measure() {
    // Without the trail: the baseline scene and its first frame.
    let mut base = scene_app((640, 360), false);
    let t = Instant::now();
    base.update();
    let base_first = t.elapsed().as_secs_f32();
    base.update();
    let (base_entities, base_tris) = census(&mut base.world);

    let mut app = scene_app((640, 360), true);
    let t = Instant::now();
    app.update();
    let first = t.elapsed().as_secs_f32();
    app.update();
    let (entities, tris) = census(&mut app.world);
    let walkers = app.world.query::<&TrailWalker>().iter(&app.world).count();

    // A second trail in the same world reuses textures and meshes.
    app.add_systems(Update, |mut commands: Commands, mut done: Local<bool>| {
        if !*done {
            *done = true;
            spawn_demo(&mut commands, DVec3::new(0.0, 0.0, 200.0));
        }
    });
    let t = Instant::now();
    app.update();
    let second = t.elapsed().as_secs_f32();
    app.update();
    let (entities2, _) = census(&mut app.world);

    println!(
        "first frame: {first:.2} s with the trail, {base_first:.2} s without -> the trail adds {:.2} s",
        first - base_first
    );
    println!(
        "a second trail in the same world: {second:.2} s frame (adds {} entities)",
        entities2 - entities
    );
    println!(
        "trail: {} entities ({walkers} walkers), {} triangles in its meshes",
        entities - base_entities,
        tris - base_tris
    );
}
