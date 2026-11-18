//! Renders a person lying prone and crawling, offscreen: going down through hands and knees,
//! lying on the forearms, frames of the low crawl, and a film of the whole thing.
//!
//! ```text
//! cargo run -p engine_anim --example prone_shots --release -- <strip|film|rifle|rifle_film> [out_dir] [width] [height]
//! ```
//!
//! * `strip`: going down (four frames), lying still, eight frames of a crawl cycle, from the
//!   side and from ahead.
//! * `film`: frames at 30 fps (`film_000.png`..): standing, down, crawling 2 m, still, up.
//! * `rifle`: a rifleman lying, crawling with the gun, aiming it lying and firing, from the
//!   side, from ahead and from above behind.
//! * `rifle_film`: frames at 30 fps (`film_000.png`..): a rifleman goes down, aims, fires
//!   three shots, crawls on, aims and fires again, and gets up.
//!
//! Needs a GPU or a software Vulkan driver (lavapipe). Time runs in fixed 1/60 s steps.

use engine_anim::AnimationPlugin;
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_render::prelude::*;
use engine_render::renderer::Renderer;
use engine_render::{RenderDisabled, capture_now};
use engine_world::WorldPlugin;
use engine_world::animation::{AnimationParams, AnimationTrigger, BodyRequest};
use engine_world::combat::{ItemClass, Loadout};
use engine_world::locomotion::MoveIntent;

const STEP: f64 = 1.0 / 60.0;

struct Stage {
    app: App,
    camera: Entity,
    parked: Option<Renderer>,
    /// Render each view once (a film, where a mesh a frame behind doesn't show).
    once: bool,
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
        Transform::default().looking_at(DVec3::new(-0.45, -0.6, -0.65), Vec3::Y),
    ));
    w.spawn((
        Transform::default(),
        MeshRenderer::new(
            Shape::Plane {
                size: [600.0, 600.0],
                subdivisions: 1,
            },
            StandardMaterial::hex(0x6b6250).with_roughness(0.95),
        ),
    ));
    let camera = w
        .spawn((
            Camera::perspective(32.0),
            Transform::from_xyz(0.0, 1.2, 5.0).looking_at(DVec3::new(0.0, 0.8, 0.0), Vec3::Y),
        ))
        .id();
    Stage {
        app,
        camera,
        parked: None,
        once: false,
    }
}

impl Stage {
    /// After everything is spawned: load the people's models, then park the renderer.
    fn ready(&mut self) {
        self.app.update();
        engine_anim::character::finish_loading(&mut self.app.world);
        for _ in 0..3 {
            self.app.update();
        }
        if let Some(d) = self.app.world.get_resource::<RenderDisabled>() {
            eprintln!("no GPU: {}", d.0);
            std::process::exit(1);
        }
        self.parked = self.app.world.remove_resource::<Renderer>();
        self.app
            .world
            .insert_resource(RenderDisabled("parked between shots".into()));
    }

    fn person(&mut self, at: DVec3, seed: u64) -> Entity {
        let t = Transform::from_translation(at);
        self.app
            .world
            .spawn((
                t,
                GlobalTransform::from(t),
                BodyRequest::human(seed),
                AnimationParams::default(),
                MoveIntent::default(),
            ))
            .id()
    }

    fn set(&mut self, e: Entity, param: &str, value: f32) {
        self.app
            .world
            .get_mut::<AnimationParams>(e)
            .unwrap()
            .set(param, value);
    }

    fn cue(&mut self, e: Entity, name: &str) {
        self.app.world.write_message(AnimationTrigger {
            entity: e,
            name: name.into(),
        });
    }

    fn run(&mut self, secs: f64) {
        for _ in 0..(secs / STEP).round() as usize {
            self.app.update();
        }
    }

    fn prone(&mut self, e: Entity, on: bool) {
        self.app
            .world
            .get_mut::<AnimationParams>(e)
            .unwrap()
            .set("prone", if on { 1.0 } else { 0.0 });
    }

    fn step(&mut self, e: Entity, way: DVec3, speed: f64) {
        let along = DVec3::new(-way.x, 0.0, -way.z);
        self.app.world.get_mut::<Transform>(e).unwrap().translation += along * speed * STEP;
        self.app.update();
    }

    fn at(&self, e: Entity) -> DVec3 {
        self.app.world.get::<Transform>(e).unwrap().translation
    }

    /// Render from `eye` at `target` (world). The first view of a frame can show a mesh
    /// where it was last drawn, so each is taken twice.
    fn shoot(&mut self, path: &std::path::Path, eye: DVec3, target: DVec3) {
        let t = Transform::from_translation(eye).looking_at(target, Vec3::Y);
        let w = &mut self.app.world;
        w.entity_mut(self.camera)
            .insert((t, GlobalTransform::from(t)));
        w.remove_resource::<RenderDisabled>();
        if let Some(r) = self.parked.take() {
            w.insert_resource(r);
        }
        let takes = if self.once { 1 } else { 2 };
        for take in 0..takes {
            match capture_now(w, path) {
                Ok(_) if take + 1 == takes => println!("{}", path.display()),
                Ok(_) => {}
                Err(e) => eprintln!("{}: {e}", path.display()),
            }
        }
        self.parked = w.remove_resource::<Renderer>();
        w.insert_resource(RenderDisabled("parked between shots".into()));
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let mode = args.next().unwrap_or_else(|| "strip".into());
    let out = std::path::PathBuf::from(args.next().unwrap_or_else(|| "prone_shots".into()));
    let width = args.next().and_then(|a| a.parse().ok()).unwrap_or(2560);
    let height = args.next().and_then(|a| a.parse().ok()).unwrap_or(1440);
    std::fs::create_dir_all(&out).expect("output directory");
    let mut s = stage(width, height);
    // The body lies from 0.6 m ahead of its spot to 1.4 m behind it.
    let side = |p: DVec3| {
        (
            p + DVec3::new(4.0, 0.8, 0.4),
            p + DVec3::new(0.0, 0.25, 0.4),
        )
    };
    let ahead = |p: DVec3| {
        (
            p + DVec3::new(1.2, 0.9, -3.2),
            p + DVec3::new(0.0, 0.25, 0.0),
        )
    };
    match mode.as_str() {
        "strip" => {
            let e = s.person(DVec3::ZERO, 23);
            s.ready();
            s.run(0.5);
            let mut shots: Vec<(String, f64)> = Vec::new();
            shots.push(("0_standing".into(), 0.0));
            s.prone(e, true);
            for (i, t) in [0.33, 0.65, 0.98, 1.3].into_iter().enumerate() {
                shots.push((format!("1_down_{i}"), t));
            }
            let mut t = 0.0;
            for (name, at) in shots {
                s.run(at - t);
                t = at;
                let p = s.at(e);
                for (view, (eye, target)) in [("side", side(p)), ("ahead", ahead(p))] {
                    s.shoot(&out.join(format!("{name}_{view}.png")), eye, target);
                }
            }
            s.run(1.0);
            let p = s.at(e);
            for (view, (eye, target)) in [("side", side(p)), ("ahead", ahead(p))] {
                s.shoot(&out.join(format!("2_lying_{view}.png")), eye, target);
            }
            // Crawl: settle into it, then eight frames of a 1.6 s cycle.
            for _ in 0..(2.0 / STEP) as usize {
                s.step(e, DVec3::Z, 0.45);
            }
            for f in 0..96 {
                if f % 12 == 0 {
                    let p = s.at(e);
                    for (view, (eye, target)) in [("side", side(p)), ("ahead", ahead(p))] {
                        s.shoot(
                            &out.join(format!("3_crawl_{}_{view}.png", f / 12)),
                            eye,
                            target,
                        );
                    }
                }
                s.step(e, DVec3::Z, 0.45);
            }
        }
        "film" => {
            let e = s.person(DVec3::ZERO, 23);
            s.ready();
            s.once = true;
            s.run(0.5);
            // (from s, prone, speed m/s, way: x left, z ahead)
            let plan = [
                (0.0, false, 0.0, DVec3::Z),
                (0.5, true, 0.0, DVec3::Z),
                (2.3, true, 0.45, DVec3::Z),
                (7.0, true, 0.0, DVec3::Z),
                (7.8, false, 0.0, DVec3::Z),
            ];
            let frames = (9.8 / STEP) as usize;
            let mut cam = s.at(e);
            for f in 0..frames {
                let t = f as f64 * STEP;
                let &(_, prone, speed, way) = plan.iter().rev().find(|p| p.0 <= t).unwrap();
                s.prone(e, prone);
                s.step(e, way, speed);
                cam += (s.at(e) - cam) * 0.08;
                if f % 2 == 0 {
                    s.shoot(
                        &out.join(format!("film_{:03}.png", f / 2)),
                        cam + DVec3::new(5.0, 1.25, 0.2),
                        cam + DVec3::new(0.0, 0.7, 0.2),
                    );
                }
            }
        }
        "rifle" => {
            let e = s.person(DVec3::ZERO, 23);
            s.app.world.entity_mut(e).insert(Loadout {
                main: ItemClass::Firearm,
                off: ItemClass::None,
                two_handed: true,
            });
            s.ready();
            s.run(0.5);
            let behind = |p: DVec3| {
                (
                    p + DVec3::new(1.0, 2.3, 3.4),
                    p + DVec3::new(0.0, 0.2, -0.2),
                )
            };
            let views = |s: &mut Stage, name: &str| {
                let p = s.at(e);
                for (view, (eye, target)) in [
                    ("side", side(p)),
                    ("ahead", ahead(p)),
                    ("behind", behind(p)),
                ] {
                    s.shoot(&out.join(format!("{name}_{view}.png")), eye, target);
                }
            };
            views(&mut s, "0_standing");
            s.prone(e, true);
            s.run(2.0);
            views(&mut s, "1_lying");
            for _ in 0..(2.0 / STEP) as usize {
                s.step(e, DVec3::Z, 0.45);
            }
            views(&mut s, "2_crawling");
            s.run(1.0);
            s.set(e, "aim", 1.0);
            s.run(1.2);
            views(&mut s, "3_aimed");
            s.cue(e, "fire");
            s.run(0.05);
            views(&mut s, "4_kick");
            s.run(0.6);
            s.set(e, "aim", 0.0);
            s.run(1.2);
            views(&mut s, "5_lowered");
        }
        "rifle_film" => {
            let e = s.person(DVec3::ZERO, 23);
            s.app.world.entity_mut(e).insert(Loadout {
                main: ItemClass::Firearm,
                off: ItemClass::None,
                two_handed: true,
            });
            s.ready();
            s.once = true;
            s.run(0.5);
            // (from s, prone, aim, speed m/s)
            let plan = [
                (0.0, false, false, 0.0),
                (0.5, true, false, 0.0),
                (2.8, true, true, 0.0),
                (5.6, true, false, 0.0),
                (6.4, true, false, 0.45),
                (9.0, true, false, 0.0),
                (9.6, true, true, 0.0),
                (11.6, false, false, 0.0),
            ];
            let shots = [3.6, 4.2, 4.8, 10.6];
            let frames = (13.6 / STEP) as usize;
            let mut cam = s.at(e);
            for f in 0..frames {
                let t = f as f64 * STEP;
                let &(_, prone, aim, speed) = plan.iter().rev().find(|p| p.0 <= t).unwrap();
                s.prone(e, prone);
                s.set(e, "aim", if aim { 1.0 } else { 0.0 });
                if shots.iter().any(|&at| (t - at).abs() < STEP * 0.5) {
                    s.cue(e, "fire");
                }
                s.step(e, DVec3::Z, speed);
                cam += (s.at(e) - cam) * 0.08;
                if f % 2 == 0 {
                    s.shoot(
                        &out.join(format!("film_{:03}.png", f / 2)),
                        cam + DVec3::new(3.6, 1.0, -1.6),
                        cam + DVec3::new(0.0, 0.45, 0.2),
                    );
                }
            }
        }
        other => eprintln!("unknown mode {other}"),
    }
}
