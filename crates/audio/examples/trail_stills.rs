//! Close-up stills of every stretch of the footstep trail, for judging the ground and props:
//! one view from eye height (1.7 m) along the path and one low three-quarter view from the
//! side, plus a wide view of the whole loop. Same scene as `trail_recording` (sun, grass
//! plane, trees).
//!
//! ```text
//! cargo run -p engine_audio --release --example trail_stills -- [out_dir]
//!     [--only grass,mud,wide] [--size 1280x720] [--walkers] [--panels]
//!     [--wide eye_x,eye_y,eye_z,at_x,at_y,at_z]
//! cargo run -p engine_audio --example trail_stills -- --measure
//! ```
//!
//! Writes `ground_<surface>.png` (the two views stacked) and `ground_wide.png`. `--panels`
//! also keeps each view as its own file. `--measure` builds the trail twice (a cold and a
//! warm spawn) and prints how long `spawn_demo` and the first frame take, and how many
//! entities and triangles the trail adds; it renders nothing.
//! Headless rendering needs a GPU or lavapipe (`mesa-vulkan-drivers`).

use std::path::{Path, PathBuf};
use std::time::Instant;

use engine_audio::demo::{STRETCHES, TrailWalker, spawn_demo, stretch_start, trail_point};
use engine_audio::{AudioOutput, AudioPlugin, AudioSettings};
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_render::{
    Atmosphere, Camera, DirectionalLight, Exposure, Mesh, MeshRenderer, RenderPlugin, ResolvedMesh,
    Screenshots, Shape, StandardMaterial,
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
    if !o.walkers {
        let walkers: Vec<Entity> = app
            .world
            .query_filtered::<Entity, With<TrailWalker>>()
            .iter(&app.world)
            .collect();
        for e in walkers {
            app.world.despawn(e);
        }
    }
    // Let shaders compile and the first uploads happen.
    app.update();

    let mut shots: Vec<(String, Vec<View>)> = Vec::new();
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
        shots.push((name.to_string(), vec![eye, low]));
    }
    shots.push((
        "wide".into(),
        vec![(
            DVec3::new(o.wide[0], o.wide[1], o.wide[2]),
            DVec3::new(o.wide[3], o.wide[4], o.wide[5]),
            52.0,
        )],
    ));

    for (name, views) in shots {
        if let Some(only) = &o.only
            && !only.contains(&name)
        {
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
                        ev100: if name == "concrete" { 13.6 } else { 14.6 },
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
    let grass = StandardMaterial::hex(0x55743a).with_roughness(0.95);
    commands.spawn((
        Transform::from_translation(origin),
        MeshRenderer::new(
            Shape::Plane {
                size: [800.0, 800.0],
                subdivisions: 1,
            },
            grass,
        ),
    ));
    // The trees of `trail_recording`.
    let bark = StandardMaterial::hex(0x5a4030).with_roughness(0.9);
    let leaves = [0x3f6b2a, 0x557f2c, 0x2f5a2f].map(|c| {
        StandardMaterial::hex(c)
            .with_roughness(0.7)
            .with_subsurface(0.45)
            .with_wind_sway(0.3)
    });
    let mut seed = 0x9E37_79B9_7F4A_7C15u64;
    let mut rnd = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed >> 11) as f64 / (1u64 << 53) as f64
    };
    for i in 0..60 {
        let a = rnd() * std::f64::consts::TAU;
        let p = if i < 6 {
            DVec3::new((rnd() - 0.5) * 26.0, 0.0, (rnd() - 0.5) * 3.0)
        } else {
            let r = 16.0 + rnd() * 34.0;
            DVec3::new(a.cos() * (r + 22.0), 0.0, a.sin() * r)
        };
        let p = origin + p;
        let h = 3.5 + rnd() as f32 * 4.5;
        commands.spawn((
            Transform::from_translation(p + DVec3::new(0.0, h as f64 * 0.5, 0.0)),
            MeshRenderer::new(
                Shape::Cylinder {
                    radius: 0.1 + h * 0.015,
                    height: h,
                    segments: 10,
                },
                bark,
            ),
        ));
        let crown = 1.1 + h * 0.2;
        for (k, (dx, dy, dz, r)) in [
            (0.0, 0.2, 0.0, 1.0),
            (0.45, 0.75, -0.25, 0.7),
            (-0.4, 0.55, 0.35, 0.75),
        ]
        .into_iter()
        .enumerate()
        {
            let c = DVec3::new(dx, dy, dz) * crown as f64;
            commands.spawn((
                Transform::from_translation(p + DVec3::new(0.0, h as f64, 0.0) + c)
                    .with_scale(Vec3::new(1.0, 0.85, 1.0)),
                MeshRenderer::new(
                    Shape::Sphere {
                        radius: crown * r as f32,
                        segments: 18,
                    },
                    leaves[(i + k) % 3],
                ),
            ));
        }
    }
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
