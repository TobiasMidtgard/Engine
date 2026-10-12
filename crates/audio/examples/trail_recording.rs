//! Film the footstep trail: picture and sound together, the camera following the hiker round
//! the loop. Along the way the runner overtakes in slow motion (his steps keep their pitch,
//! everyone else's drop), the hiker goes through the echoing underpass, and rain starts,
//! soaking the ground so the last stretches squelch and splash.
//!
//! ```text
//! cargo run -p engine_audio --release --example trail_recording -- [out_dir] [--secs 86]
//!     [--fps 30] [--size 1280x720] [--no-video]
//! ```
//!
//! Writes `trail.wav` (exactly as mixed), `frames/00000.png…` and, when `ffmpeg` is installed,
//! `trail.mp4` (sound loudness-normalized for listening).
//! Headless rendering needs a GPU or lavapipe (`mesa-vulkan-drivers`).

use std::path::PathBuf;

use engine_audio::demo::{TRAIL_LENGTH, TrailWalker, spawn_demo, stretch_start};
use engine_audio::{Audio, AudioListener, AudioOutput, AudioPlugin, AudioSettings};
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_render::{
    Atmosphere, Camera, DirectionalLight, MeshRenderer, RenderPlugin, Screenshots, Shape,
    StandardMaterial,
};
use engine_world::{PrecipitationKind, SurfaceLayers, WeatherState, WorldPlugin};

struct Options {
    out: PathBuf,
    secs: f32,
    fps: u32,
    size: (u32, u32),
    video: bool,
}

fn options() -> Options {
    let mut o = Options {
        out: PathBuf::from("trail_recording"),
        secs: 86.0,
        fps: 30,
        size: (1280, 720),
        video: true,
    };
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < args.len() {
        let next = args.get(i + 1).cloned().unwrap_or_default();
        match args[i].as_str() {
            "--secs" => {
                o.secs = next.parse().expect("--secs takes seconds");
                i += 1;
            }
            "--fps" => {
                o.fps = next.parse().expect("--fps takes a number");
                i += 1;
            }
            "--size" => {
                let (w, h) = next.split_once('x').expect("--size takes WxH");
                o.size = (w.parse().expect("width"), h.parse().expect("height"));
                i += 1;
            }
            "--no-video" => o.video = false,
            a if !a.starts_with("--") => o.out = PathBuf::from(a),
            a => panic!("unknown option {a}"),
        }
        i += 1;
    }
    o
}

/// The hiker the camera follows (the one who doesn't ignore slow motion and walks on two
/// feet at walking pace).
#[derive(Component)]
struct Followed;

#[derive(Resource)]
struct Script {
    origin: DVec3,
    rain_at: f64,
    slow_at: f64,
    slow_done: bool,
}

fn main() {
    engine_core::log::init("info,wgpu=warn,naga=warn");
    let o = options();
    let frames_dir = o.out.join("frames");
    std::fs::create_dir_all(&frames_dir).expect("create output folder");
    if o.video {
        for e in std::fs::read_dir(&frames_dir)
            .into_iter()
            .flatten()
            .flatten()
        {
            let _ = std::fs::remove_file(e.path());
        }
    }

    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = o.size.0;
        w.height = o.size.1;
    }
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / o.fps as f64,
        });
    app.insert_resource(AudioSettings {
        output: AudioOutput::Offline,
        ..Default::default()
    });
    if o.video {
        app.add_plugins(RenderPlugin);
    }
    app.add_plugins((WorldPlugin, AudioPlugin));

    let origin = DVec3::ZERO;
    // The runner (30 m ahead at 4 m/s) catches the hiker (1.4 m/s) after 86 m.
    let catch_up = (TRAIL_LENGTH - 30.0) / (4.0 - 1.4);
    app.insert_resource(Script {
        origin,
        rain_at: stretch_start("metal").unwrap_or(48.0) / 1.4,
        slow_at: catch_up - 1.6,
        slow_done: false,
    });
    {
        let mut weather = app.world.resource_mut::<WeatherState>();
        weather.wind = Vec3::new(1.5, 0.0, 0.8);
        weather.cloud_cover = 0.3;
    }
    if let Some(mut a) = app.world.get_resource_mut::<Atmosphere>() {
        a.ground_color = [0.09, 0.18, 0.04];
    }
    app.add_systems(Startup, setup)
        .add_systems(Update, (tag_hiker, script))
        .add_systems(PostUpdate, follow_camera.before(UpdateSet::AfterPropagate));

    let total = (o.secs * o.fps as f32) as usize;
    let wav = o.out.join("trail.wav");
    app.update();
    app.world
        .resource_mut::<Audio>()
        .start_recording(&wav, o.secs + 1.0)
        .expect("start recording");
    let started = std::time::Instant::now();
    for f in 0..total {
        if o.video {
            app.world
                .resource_mut::<Screenshots>()
                .request(frames_dir.join(format!("{f:05}.png")));
        }
        app.update();
        if f % (o.fps as usize * 5) == 0 {
            let t = f as f32 / o.fps as f32;
            engine_core::log::info!(
                "{t:5.1} s of {:.0} ({:.1} s per second of film)",
                o.secs,
                started.elapsed().as_secs_f32() / t.max(0.1)
            );
        }
    }
    let (path, secs) = app
        .world
        .resource_mut::<Audio>()
        .stop_recording()
        .expect("write the recording");
    println!("sound: {} ({secs:.1} s)", path.display());

    if o.video {
        let mp4 = o.out.join("trail.mp4");
        let status = std::process::Command::new("ffmpeg")
            .args([
                "-y",
                "-loglevel",
                "error",
                "-framerate",
                &o.fps.to_string(),
                "-i",
            ])
            .arg(frames_dir.join("%05d.png"))
            .arg("-i")
            .arg(&wav)
            .args([
                "-c:v",
                "libx264",
                "-preset",
                "slow",
                "-crf",
                "20",
                "-pix_fmt",
                "yuv420p",
                "-c:a",
                "aac",
                "-b:a",
                "192k",
                "-shortest",
                // Bring the level up for listening on a laptop (trail.wav stays as mixed).
                "-af",
                "loudnorm=I=-18:TP=-1.5:LRA=14",
            ])
            .arg(&mp4)
            .status();
        match status {
            Ok(s) if s.success() => println!("video: {}", mp4.display()),
            _ => println!(
                "frames in {}; install ffmpeg to join them into a video",
                frames_dir.display()
            ),
        }
    }
}

fn setup(mut commands: Commands, script: Res<Script>) {
    let origin = script.origin;
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
    // Trees round the trail, so there is something to walk past.
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
            // A few trees in the middle of the loop.
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
    spawn_demo(&mut commands, origin);
    let mut cam = Camera::perspective(50.0);
    if let engine_render::Exposure::Auto { speed, .. } = &mut cam.exposure {
        *speed = 6.0;
    }
    commands.spawn((cam, Transform::from_xyz(-24.0, 3.0, -14.0)));
}

/// Find the hiker once the demo has spawned and give it ears.
fn tag_hiker(
    mut commands: Commands,
    walkers: Query<(Entity, &TrailWalker), Without<Followed>>,
    followed: Query<(), With<Followed>>,
) {
    if !followed.is_empty() {
        return;
    }
    for (e, w) in &walkers {
        if w.is_hiker() {
            commands.entity(e).insert(Followed).with_children(|c| {
                c.spawn((
                    AudioListener { priority: 10 },
                    Transform::from_xyz(0.0, 1.65, 0.0),
                ));
            });
            return;
        }
    }
}

fn script(
    time: Res<Time>,
    mut s: ResMut<Script>,
    mut weather: ResMut<WeatherState>,
    mut layers: ResMut<SurfaceLayers>,
    mut dilation: ResMut<TimeDilation>,
) {
    let t = time.real_elapsed_secs_f64();
    if !s.slow_done && t >= s.slow_at {
        s.slow_done = true;
        dilation.pulse("trail", 0.25, 3.0);
    }
    // Rain moves in: drizzle building to a downpour, the ground darkening and pooling.
    let rain = ((t - s.rain_at) / 10.0).clamp(0.0, 1.0) as f32;
    if rain > 0.0 {
        weather.precipitation_kind = PrecipitationKind::Rain;
        weather.precipitation_mm_h = 0.5 + 11.5 * rain;
        weather.cloud_cover = 0.3 + 0.65 * rain;
        weather.cloud_density = 0.4 + 0.5 * rain;
        let wet = ((t - s.rain_at) / 16.0).clamp(0.0, 1.0) as f32;
        layers.grid.fill(engine_world::SurfaceCell {
            wetness: wet,
            puddle_m: 0.0,
            snow_m: 0.0,
            scorch: 0.0,
        });
    }
}

fn follow_camera(
    time: Res<Time>,
    hiker: Query<&TrailWalker, With<Followed>>,
    mut cams: Query<&mut Transform, With<Camera>>,
    mut eye: Local<Option<DVec3>>,
    mut look: Local<Option<DVec3>>,
) {
    let Ok(w) = hiker.single() else {
        return;
    };
    let (want_eye, want_look) = w.follow_view();
    let k = 1.0 - (-time.real_delta_secs() as f64 * 2.5).exp();
    let e = eye.map_or(want_eye, |e| e + (want_eye - e) * k);
    let l = look.map_or(want_look, |l| l + (want_look - l) * k);
    *eye = Some(e);
    *look = Some(l);
    for mut t in &mut cams {
        *t = Transform::from_translation(e).looking_at(l, Vec3::Y);
    }
}
