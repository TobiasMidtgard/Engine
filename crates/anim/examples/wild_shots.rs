//! Renders the wild animals (deer, wolf, boar, rabbit) offscreen, asked for as NPC AI asks.
//!
//! ```text
//! cargo run -p engine_anim --example wild_shots --release -- <lineup|close|actions|gaits> [out_dir] [width] [height]
//! ```
//!
//! * `lineup`: a stag, a hind, a fawn, a wolf, a boar, a piglet and a rabbit side by side.
//! * `close`: each one close up, three-quarter on.
//! * `actions`: the held poses and the one-shots (graze, lie, alert, rear, howl, fallen).
//! * `gaits`: frames of each one walking, trotting and running past.
//!
//! Needs a GPU or a software Vulkan driver (lavapipe). Time runs in fixed 1/60 s steps, so the
//! pictures are the same on every run.

use engine_anim::AnimationPlugin;
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_render::prelude::*;
use engine_render::renderer::Renderer;
use engine_render::{RenderDisabled, capture_now};
use engine_world::WorldPlugin;
use engine_world::animation::{AnimationParams, AnimationTrigger, BodyRequest};

const STEP: f64 = 1.0 / 60.0;

struct Stage {
    app: App,
    camera: Entity,
    parked: Option<Renderer>,
}

fn stage(width: u32, height: u32) -> Stage {
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = width;
        w.height = height;
    }
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual { step_secs: STEP });
    app.add_plugins((WorldPlugin, RenderPlugin, AnimationPlugin));
    let w = &mut app.world;
    w.spawn((
        DirectionalLight {
            illuminance: 90_000.0,
            ..Default::default()
        },
        Transform::default().looking_at(DVec3::new(-0.45, -0.55, -0.7), Vec3::Y),
    ));
    // A meadow-coloured ground.
    w.spawn((
        Transform::default(),
        MeshRenderer::new(
            Shape::Plane {
                size: [400.0, 400.0],
                subdivisions: 1,
            },
            StandardMaterial::hex(0x4f5a2e).with_roughness(0.97),
        ),
    ));
    let camera = w
        .spawn((
            Camera::perspective(30.0),
            Transform::from_xyz(0.0, 1.2, 6.0).looking_at(DVec3::new(0.0, 0.6, 0.0), Vec3::Y),
        ))
        .id();
    for _ in 0..3 {
        app.update();
    }
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("no GPU: {}", d.0);
        std::process::exit(1);
    }
    let parked = app.world.remove_resource::<Renderer>();
    app.world
        .insert_resource(RenderDisabled("parked between shots".into()));
    Stage {
        app,
        camera,
        parked,
    }
}

impl Stage {
    fn animal(
        &mut self,
        kind: &str,
        height: Option<f32>,
        seed: u64,
        at: DVec3,
        yaw: f32,
    ) -> Entity {
        let t =
            Transform::from_translation(at).with_rotation(Quat::from_rotation_y(yaw.to_radians()));
        let mut r = BodyRequest::new(kind).with_seed(seed);
        r.height = height;
        self.app
            .world
            .spawn((t, GlobalTransform::from(t), r, AnimationParams::default()))
            .id()
    }

    fn run(&mut self, secs: f64) {
        for _ in 0..(secs / STEP).round() as usize {
            self.app.update();
        }
    }

    fn set(&mut self, e: Entity, name: &str, value: f32) {
        self.app
            .world
            .get_mut::<AnimationParams>(e)
            .unwrap()
            .set(name, value);
    }

    fn trigger(&mut self, e: Entity, name: &str) {
        self.app.world.write_message(AnimationTrigger {
            entity: e,
            name: name.into(),
        });
    }

    fn at(&self, e: Entity) -> DVec3 {
        self.app.world.get::<Transform>(e).unwrap().translation
    }

    /// Render from `eye` at `target` (world).
    fn shoot(&mut self, path: &std::path::Path, eye: DVec3, target: DVec3) {
        let t = Transform::from_translation(eye).looking_at(target, Vec3::Y);
        let w = &mut self.app.world;
        w.entity_mut(self.camera)
            .insert((t, GlobalTransform::from(t)));
        w.remove_resource::<RenderDisabled>();
        if let Some(r) = self.parked.take() {
            w.insert_resource(r);
        }
        match capture_now(w, path) {
            Ok(_) => println!("{}", path.display()),
            Err(e) => eprintln!("{}: {e}", path.display()),
        }
        self.parked = w.remove_resource::<Renderer>();
        w.insert_resource(RenderDisabled("parked between shots".into()));
    }
}

/// (name, kind, height, seed, how far the camera stands back, the height it looks at).
type Cast = (&'static str, &'static str, Option<f32>, u64, f64, f64);

const CAST: [Cast; 7] = [
    ("stag", "deer", Some(1.32), 7, 4.6, 0.95),
    ("hind", "deer", None, 3, 4.2, 0.85),
    ("fawn", "deer", Some(0.6), 5, 2.4, 0.45),
    ("wolf", "wolf", None, 4, 3.0, 0.55),
    ("boar", "boar", None, 6, 3.0, 0.5),
    ("piglet", "boar", Some(0.425), 8, 1.8, 0.28),
    ("rabbit", "rabbit", None, 2, 0.9, 0.13),
];

/// (shot, kind, height, param or trigger, is a trigger, when (s), back, look).
type Action = (
    &'static str,
    &'static str,
    Option<f32>,
    &'static str,
    bool,
    f64,
    f64,
    f64,
);

fn main() {
    let mut args = std::env::args().skip(1);
    let mode = args.next().unwrap_or_else(|| "lineup".into());
    let out = std::path::PathBuf::from(args.next().unwrap_or_else(|| "wild_shots".into()));
    let width = args.next().and_then(|a| a.parse().ok()).unwrap_or(1280);
    let height = args.next().and_then(|a| a.parse().ok()).unwrap_or(720);
    std::fs::create_dir_all(&out).expect("output directory");
    let mut s = stage(width, height);
    match mode.as_str() {
        "lineup" => {
            let xs = [-4.4, -2.5, -1.1, 0.3, 1.7, 3.1, 3.95];
            for (i, (_, kind, h, seed, ..)) in CAST.iter().enumerate() {
                s.animal(kind, *h, *seed, DVec3::new(xs[i], 0.0, 0.0), -90.0);
            }
            s.run(1.5);
            s.shoot(
                &out.join("lineup.png"),
                DVec3::new(-0.3, 1.3, 9.5),
                DVec3::new(-0.3, 0.6, 0.0),
            );
        }
        "close" => {
            for (i, (name, kind, h, seed, back, look)) in CAST.iter().enumerate() {
                let at = DVec3::new(30.0 * i as f64, 0.0, 0.0);
                s.animal(kind, *h, *seed, at, -60.0);
                s.run(1.2);
                let eye = at + DVec3::new(back * 0.55, look * 1.25, back * 0.85);
                s.shoot(
                    &out.join(format!("close_{name}.png")),
                    eye,
                    at + DVec3::new(0.0, *look, 0.0),
                );
                let eye = at + DVec3::new(-back * 0.7, look * 1.1, -back * 0.55);
                s.shoot(
                    &out.join(format!("close_{name}_front.png")),
                    eye,
                    at + DVec3::new(0.0, look * 1.1, -0.2 * back / 4.0),
                );
            }
        }
        "actions" => {
            // (shot, kind, height, param or trigger, is a trigger, when (s), back, look)
            let shots: [Action; 11] = [
                ("deer_graze", "deer", None, "graze", false, 3.0, 4.2, 0.6),
                ("deer_lie", "deer", None, "lie", false, 3.0, 4.2, 0.5),
                ("deer_alert", "deer", None, "alert", false, 1.5, 4.2, 0.9),
                ("stag_rear", "deer", Some(1.32), "rear", true, 0.6, 5.0, 1.1),
                ("deer_downed", "deer", None, "downed", false, 2.5, 4.2, 0.3),
                ("wolf_howl", "wolf", None, "howl", true, 2.6, 3.0, 0.6),
                ("wolf_lie", "wolf", None, "lie", false, 3.0, 3.0, 0.35),
                ("boar_root", "boar", None, "graze", false, 2.2, 3.0, 0.4),
                ("boar_alert", "boar", None, "alert", false, 1.5, 3.0, 0.55),
                (
                    "rabbit_alert",
                    "rabbit",
                    None,
                    "alert",
                    false,
                    1.5,
                    0.9,
                    0.17,
                ),
                (
                    "rabbit_graze",
                    "rabbit",
                    None,
                    "graze",
                    false,
                    2.2,
                    0.9,
                    0.1,
                ),
            ];
            for (i, (name, kind, h, what, trigger, when, back, look)) in shots.iter().enumerate() {
                let at = DVec3::new(30.0 * i as f64, 0.0, 0.0);
                let e = s.animal(kind, *h, i as u64 + 1, at, -90.0);
                s.run(0.6);
                if *trigger {
                    s.trigger(e, what);
                } else {
                    s.set(e, what, 1.0);
                }
                s.run(*when);
                let eye = at + DVec3::new(back * 0.25, look + back * 0.12, *back);
                s.shoot(
                    &out.join(format!("{name}.png")),
                    eye,
                    at + DVec3::new(0.0, *look, 0.0),
                );
            }
        }
        "gaits" => {
            // (name, kind, speed m/s, back, look)
            let runs = [
                ("deer_walk", "deer", 1.0, 5.0, 0.8),
                ("deer_trot", "deer", 2.6, 5.0, 0.8),
                ("deer_gallop", "deer", 9.0, 6.0, 0.8),
                ("wolf_trot", "wolf", 2.8, 3.6, 0.5),
                ("wolf_gallop", "wolf", 10.0, 4.2, 0.5),
                ("boar_trot", "boar", 2.0, 3.6, 0.45),
                ("boar_gallop", "boar", 6.0, 4.0, 0.45),
                ("rabbit_hop", "rabbit", 0.6, 1.1, 0.12),
                ("rabbit_bound", "rabbit", 4.0, 1.6, 0.12),
            ];
            for (i, (name, kind, speed, back, look)) in runs.iter().enumerate() {
                let start = DVec3::new(40.0 * i as f64, 0.0, 0.0);
                // Running along −X, its right side to the camera... its left: facing −X.
                let e = s.animal(kind, None, i as u64 + 1, start, 90.0);
                let mut frames = 0;
                for f in 0..150 {
                    s.app.world.get_mut::<Transform>(e).unwrap().translation.x -= speed * STEP;
                    s.app.update();
                    if f >= 90 && f % 4 == 0 && frames < 8 {
                        let p = s.at(e);
                        let eye = p + DVec3::new(-0.15 * back, look + 0.1 * back, *back);
                        s.shoot(
                            &out.join(format!("{name}_{frames}.png")),
                            eye,
                            p + DVec3::new(0.0, *look, 0.0),
                        );
                        frames += 1;
                    }
                }
            }
        }
        other => eprintln!("unknown mode {other}"),
    }
}
