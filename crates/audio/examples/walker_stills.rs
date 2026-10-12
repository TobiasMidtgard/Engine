//! Close-ups of the footstep trail's walkers: each one from the side and at three quarters,
//! and a contact sheet of eight frames through one gait cycle (the camera travels with the
//! walker; the coloured rings mark where `GroundContact`s landed).
//!
//! ```text
//! cargo run -p engine_audio --release --example walker_stills -- [out_dir]
//!     [--only hiker|runner|dog|horse] [--size 1280x720] [--no-cycle] [--no-stills]
//!     [--at <meters along the trail>] [--fixed] [--detail]
//! ```
//!
//! Writes `bodies_<walker>.png` (side and three-quarter views side by side) and
//! `bodies_<walker>_cycle.png`, and prints each walker's triangle count. `--fixed` keeps the
//! camera still for the cycle, so planted feet should stay put on screen; `--detail` adds
//! close-ups (`detail_<walker>_<n>.png`) of the head, feet and back. Headless rendering needs a GPU or lavapipe
//! (`mesa-vulkan-drivers`).

use std::path::{Path, PathBuf};

use engine_assets::Assets;
use engine_audio::demo::{TrailWalker, spawn_demo};
use engine_audio::{AudioOutput, AudioPlugin, AudioSettings};
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_render::{
    Atmosphere, Camera, DirectionalLight, Exposure, Mesh, MeshRef, MeshRenderer, RenderPlugin,
    Screenshots, Shape, StandardMaterial,
};
use engine_world::WorldPlugin;

struct Options {
    out: PathBuf,
    only: Option<String>,
    size: (u32, u32),
    cycle: bool,
    stills: bool,
    at: Option<f64>,
    fixed: bool,
    detail: bool,
}

fn options() -> Options {
    let mut o = Options {
        out: PathBuf::from("walker_stills"),
        only: None,
        size: (1280, 720),
        cycle: true,
        stills: true,
        at: None,
        fixed: false,
        detail: false,
    };
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < args.len() {
        let next = args.get(i + 1).cloned().unwrap_or_default();
        match args[i].as_str() {
            "--only" => {
                o.only = Some(next);
                i += 1;
            }
            "--size" => {
                let (w, h) = next.split_once('x').expect("--size takes WxH");
                o.size = (w.parse().expect("width"), h.parse().expect("height"));
                i += 1;
            }
            "--at" => {
                o.at = Some(next.parse().expect("--at takes meters"));
                i += 1;
            }
            "--no-cycle" => o.cycle = false,
            "--no-stills" => o.stills = false,
            "--fixed" => o.fixed = true,
            "--detail" => o.detail = true,
            a if !a.starts_with("--") => o.out = PathBuf::from(a),
            a => panic!("unknown option {a}"),
        }
        i += 1;
    }
    o
}

/// Which walker the camera frames, and from where.
#[derive(Resource, Clone, Copy)]
struct Shot {
    target: Option<Entity>,
    /// Camera direction from the walker in its own frame (x right, y up, z back), meters.
    offset: Vec3,
    /// Height of the point the camera looks at.
    look_y: f32,
    /// How far ahead of the walker's centre to look.
    ahead: f32,
    /// Keep the camera where it is (it still aims at `fixed_look`).
    fixed: Option<(DVec3, DVec3)>,
}

struct Subject {
    name: &'static str,
    file: &'static str,
    /// Look-at height, side distance, height of the camera.
    look_y: f32,
    dist: f32,
    eye_y: f32,
    /// Close-ups for `--detail`: look-at height, how far ahead of the walker's centre,
    /// distance, angle round from the side (radians, toward the front).
    detail: [(f32, f32, f32, f32); 4],
}

const SUBJECTS: [Subject; 4] = [
    Subject {
        name: "Trail walker",
        file: "hiker",
        look_y: 0.95,
        dist: 3.0,
        eye_y: 1.25,
        detail: [
            (1.62, 0.0, 0.75, 0.0),
            (1.62, 0.0, 0.75, 1.1),
            (0.25, 0.0, 1.2, 0.5),
            (1.25, 0.0, 1.4, 2.6),
        ],
    },
    Subject {
        name: "Trail runner",
        file: "runner",
        look_y: 0.95,
        dist: 3.2,
        eye_y: 1.2,
        detail: [
            (1.6, 0.0, 0.75, 0.0),
            (1.6, 0.0, 0.75, 1.1),
            (0.25, 0.0, 1.2, 0.5),
            (1.2, 0.0, 1.4, 1.2),
        ],
    },
    Subject {
        name: "Trail dog",
        file: "dog",
        look_y: 0.36,
        dist: 1.55,
        eye_y: 0.6,
        detail: [
            (0.55, 0.45, 0.6, 0.0),
            (0.55, 0.45, 0.6, 1.0),
            (0.15, 0.0, 0.9, 0.4),
            (0.45, -0.1, 1.0, 2.6),
        ],
    },
    Subject {
        name: "Trail horse",
        file: "horse",
        look_y: 1.1,
        dist: 4.9,
        eye_y: 1.5,
        detail: [
            (1.45, 1.1, 1.3, 0.2),
            (1.45, 1.1, 1.3, 1.1),
            (0.4, 0.0, 1.8, 0.4),
            (1.2, 0.0, 2.8, 2.6),
        ],
    },
];

fn main() {
    engine_core::log::init("warn");
    let o = options();
    std::fs::create_dir_all(&o.out).expect("create output folder");
    let frames = o.out.join("frames");
    std::fs::create_dir_all(&frames).expect("create frames folder");

    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = o.size.0;
        w.height = o.size.1;
    }
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual { step_secs: 1e-4 });
    app.insert_resource(AudioSettings {
        output: AudioOutput::Offline,
        ..Default::default()
    });
    app.add_plugins((RenderPlugin, WorldPlugin, AudioPlugin));
    if let Some(mut a) = app.world.get_resource_mut::<Atmosphere>() {
        a.ground_color = [0.09, 0.18, 0.04];
    }
    app.insert_resource(Shot {
        target: None,
        offset: Vec3::Z,
        look_y: 1.0,
        ahead: 0.0,
        fixed: None,
    });
    app.add_systems(Startup, setup)
        .add_systems(PostUpdate, aim.before(UpdateSet::AfterPropagate));
    app.update();
    app.update();

    for s in &SUBJECTS {
        if o.only.as_deref().is_some_and(|n| n != s.file) {
            continue;
        }
        let Some(target) = find(&mut app, s.name) else {
            eprintln!("no walker named {}", s.name);
            continue;
        };
        println!("{}: {} triangles", s.file, triangles(&mut app, target));
        // Put this walker on a plain stretch and everyone else far round the loop.
        let at = o.at.unwrap_or(16.0);
        place(&mut app, target, at);
        if o.stills {
            let mut halves = Vec::new();
            for (k, offset) in [
                Vec3::new(s.dist, s.eye_y - s.look_y, 0.0),
                Vec3::new(s.dist * 0.72, s.eye_y - s.look_y + 0.1, -s.dist * 0.72),
            ]
            .into_iter()
            .enumerate()
            {
                *app.world.resource_mut::<Shot>() = Shot {
                    target: Some(target),
                    offset,
                    look_y: s.look_y,
                    ahead: 0.0,
                    fixed: None,
                };
                let path = frames.join(format!("{}_{k}.png", s.file));
                shoot(&mut app, &path, 1e-4);
                halves.push(path);
            }
            let out = o.out.join(format!("bodies_{}.png", s.file));
            sheet(&halves, 2, 1.0, &out);
            println!("{}", out.display());
        }
        if o.detail {
            for (k, &(look_y, ahead, dist, angle)) in s.detail.iter().enumerate() {
                *app.world.resource_mut::<Shot>() = Shot {
                    target: Some(target),
                    offset: Vec3::new(dist * angle.cos(), 0.05, -dist * angle.sin()),
                    look_y,
                    ahead,
                    fixed: None,
                };
                let path = o.out.join(format!("detail_{}_{k}.png", s.file));
                shoot(&mut app, &path, 1e-4);
                println!("{}", path.display());
            }
        }
        if o.cycle {
            let (stride, speed) = {
                let w = app.world.get::<TrailWalker>(target).expect("walker");
                (w.feet as f32 * w.step_len, w.speed)
            };
            place(&mut app, target, at);
            let offset = Vec3::new(s.dist * 0.95, s.eye_y - s.look_y - 0.1, 0.0);
            *app.world.resource_mut::<Shot>() = Shot {
                target: Some(target),
                offset,
                look_y: s.look_y,
                ahead: 0.0,
                fixed: None,
            };
            // Settle on the first frame, then freeze the camera there if asked.
            shoot(&mut app, &frames.join("settle.png"), 1e-4);
            if o.fixed {
                let (eye, look) = camera_now(&mut app);
                app.world.resource_mut::<Shot>().fixed = Some((eye, look));
            }
            let mut paths = Vec::new();
            for k in 0..8 {
                let path = frames.join(format!("{}_cycle{k}.png", s.file));
                // Each frame moves the walker an eighth of a stride.
                shoot(&mut app, &path, (stride / 8.0 / speed) as f64);
                paths.push(path);
            }
            let out = o.out.join(format!("bodies_{}_cycle.png", s.file));
            sheet(&paths, 4, 0.5, &out);
            println!("{}", out.display());
        }
    }
}

fn setup(mut commands: Commands) {
    commands.spawn((
        DirectionalLight::default(),
        Transform::default().looking_at(DVec3::new(-0.35, -0.55, -0.75), Vec3::Y),
    ));
    commands.spawn((
        Transform::default(),
        MeshRenderer::new(
            Shape::Plane {
                size: [400.0, 400.0],
                subdivisions: 1,
            },
            StandardMaterial::hex(0x55743a).with_roughness(0.95),
        ),
    ));
    spawn_demo(&mut commands, DVec3::ZERO);
    commands.spawn((
        Camera::perspective(40.0).with_exposure(Exposure::Manual { ev100: 14.6 }),
        Transform::from_xyz(0.0, 2.0, 10.0),
    ));
}

fn find(app: &mut App, name: &str) -> Option<Entity> {
    let mut q = app.world.query::<(Entity, &Name, &TrailWalker)>();
    q.iter(&app.world)
        .find(|(_, n, _)| n.as_str() == name)
        .map(|(e, _, _)| e)
}

/// How many triangles a walker's body has (fur shells and hair cards included).
fn triangles(app: &mut App, walker: Entity) -> usize {
    let kids: Vec<Entity> = app
        .world
        .get::<Children>(walker)
        .map(|c| c[..].to_vec())
        .unwrap_or_default();
    let meshes = app.world.resource::<Assets<Mesh>>();
    kids.iter()
        .filter_map(|&e| match &app.world.get::<MeshRenderer>(e)?.mesh {
            MeshRef::Handle(h) => meshes.get(*h).map(|m| m.indices.len() / 3),
            _ => None,
        })
        .sum()
}

/// Put `target` at `at` meters along the trail and the other walkers well away.
fn place(app: &mut App, target: Entity, at: f64) {
    let mut q = app.world.query::<(Entity, &mut TrailWalker)>();
    let mut k = 0.0;
    for (e, mut w) in q.iter_mut(&mut app.world) {
        if e == target {
            w.along = at;
        } else {
            k += 1.0;
            w.along = at + 40.0 + 18.0 * k;
        }
    }
}

/// Step the world by `dt` seconds and save the frame.
fn shoot(app: &mut App, path: &Path, dt: f64) {
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual { step_secs: dt });
    app.world
        .resource_mut::<Screenshots>()
        .request(path.to_path_buf());
    app.update();
}

fn camera_now(app: &mut App) -> (DVec3, DVec3) {
    let mut q = app.world.query_filtered::<&Transform, With<Camera>>();
    let t = q.single(&app.world).expect("camera");
    (t.translation, t.translation + t.forward().as_dvec3() * 3.0)
}

fn aim(
    shot: Res<Shot>,
    walkers: Query<&Transform, (With<TrailWalker>, Without<Camera>)>,
    mut cams: Query<&mut Transform, With<Camera>>,
) {
    if let Some((eye, look)) = shot.fixed {
        for mut t in &mut cams {
            *t = Transform::from_translation(eye).looking_at(look, Vec3::Y);
        }
        return;
    }
    let Some(w) = shot.target.and_then(|e| walkers.get(e).ok()) else {
        return;
    };
    let base = w.translation;
    let look = base + (w.rotation * Vec3::new(0.0, shot.look_y, -shot.ahead)).as_dvec3();
    let eye = look + (w.rotation * shot.offset).as_dvec3();
    for mut t in &mut cams {
        *t = Transform::from_translation(eye).looking_at(look, Vec3::Y);
    }
}

/// Tile images `cols` across (scaled by `scale`) into one PNG.
fn sheet(paths: &[PathBuf], cols: u32, scale: f32, out: &Path) {
    let imgs: Vec<image::RgbaImage> = paths
        .iter()
        .map(|p| image::open(p).expect("frame").to_rgba8())
        .map(|i| {
            if scale == 1.0 {
                i
            } else {
                let (w, h) = (
                    (i.width() as f32 * scale) as u32,
                    (i.height() as f32 * scale) as u32,
                );
                image::imageops::resize(&i, w, h, image::imageops::FilterType::Triangle)
            }
        })
        .collect();
    let (w, h) = (imgs[0].width(), imgs[0].height());
    let rows = (imgs.len() as u32).div_ceil(cols);
    let mut canvas = image::RgbaImage::new(w * cols, h * rows);
    for (i, img) in imgs.iter().enumerate() {
        let (x, y) = ((i as u32 % cols) * w, (i as u32 / cols) * h);
        image::imageops::overlay(&mut canvas, img, x as i64, y as i64);
    }
    canvas.save(out).expect("save sheet");
}
