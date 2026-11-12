//! Renders people crouching, offscreen: behind a waist-high wall (standing for comparison,
//! crouched, holding a bow drawn and aiming a rifle crouched), and a film of one dropping
//! into a crouch, creeping, hurrying low, stepping aside and standing up.
//!
//! ```text
//! cargo run -p engine_anim --example crouch_shots --release -- <wall|film|gaits> [out_dir] [width] [height]
//! ```
//!
//! * `wall`: each pose from the side and from behind, the wall 1 m high.
//! * `film`: frames at 30 fps of the whole sequence (`film_000.png`..), the camera alongside.
//! * `gaits`: eight frames each of the crouched walk, run, back and side steps.
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
use engine_world::combat::{ItemClass, Loadout};
use engine_world::locomotion::MoveIntent;

const STEP: f64 = 1.0 / 60.0;
/// Height of the wall they crouch behind, m.
const WALL: f32 = 1.0;

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

    fn person(&mut self, at: DVec3, seed: u64, loadout: Option<Loadout>) -> Entity {
        let t = Transform::from_translation(at);
        let mut e = self.app.world.spawn((
            t,
            GlobalTransform::from(t),
            BodyRequest::human(seed),
            AnimationParams::default(),
            MoveIntent::default(),
        ));
        if let Some(l) = loadout {
            e.insert(l);
        }
        e.id()
    }

    /// A stone wall `WALL` high across −Z of `at`.
    fn wall(&mut self, at: DVec3) {
        let t = Transform::from_translation(at + DVec3::new(0.0, WALL as f64 * 0.5, -0.88));
        self.app.world.spawn((
            t,
            GlobalTransform::from(t),
            MeshRenderer::new(
                Shape::Cuboid {
                    size: [2.6, WALL, 0.3],
                },
                StandardMaterial::hex(0x8c8478).with_roughness(0.9),
            ),
        ));
    }

    fn run(&mut self, secs: f64) {
        for _ in 0..(secs / STEP).round() as usize {
            self.app.update();
        }
    }

    fn crouch(&mut self, e: Entity, on: bool) {
        self.app.world.get_mut::<MoveIntent>(e).unwrap().crouch = on;
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
    let mode = args.next().unwrap_or_else(|| "wall".into());
    let out = std::path::PathBuf::from(args.next().unwrap_or_else(|| "crouch_shots".into()));
    let width = args.next().and_then(|a| a.parse().ok()).unwrap_or(2560);
    let height = args.next().and_then(|a| a.parse().ok()).unwrap_or(1440);
    std::fs::create_dir_all(&out).expect("output directory");
    let mut s = stage(width, height);
    let two_handed = |main| Loadout {
        main,
        off: ItemClass::None,
        two_handed: true,
    };
    match mode.as_str() {
        "wall" => {
            // (shot, loadout, crouched, then)
            let poses = [
                ("standing", None, false, ""),
                ("crouched", None, true, ""),
                ("bow_held", Some(two_handed(ItemClass::Bow)), true, "draw"),
                (
                    "rifle_aimed",
                    Some(two_handed(ItemClass::Firearm)),
                    true,
                    "aim",
                ),
            ];
            let people: Vec<Entity> = poses
                .iter()
                .enumerate()
                .map(|(i, (_, loadout, ..))| {
                    let at = DVec3::new(30.0 * i as f64, 0.0, 0.0);
                    s.wall(at);
                    s.person(at, 21 + i as u64, *loadout)
                })
                .collect();
            s.ready();
            s.run(0.6);
            for (&e, (_, _, crouched, _)) in people.iter().zip(&poses) {
                s.crouch(e, *crouched);
            }
            s.run(1.0);
            for (&e, (.., then)) in people.iter().zip(&poses) {
                match *then {
                    "draw" => {
                        s.app.world.write_message(AnimationTrigger {
                            entity: e,
                            name: "draw".into(),
                        });
                    }
                    "aim" => s
                        .app
                        .world
                        .get_mut::<AnimationParams>(e)
                        .unwrap()
                        .set("aim", 1.0),
                    _ => {}
                }
            }
            s.run(2.0);
            for (&e, (name, ..)) in people.iter().zip(&poses) {
                let at = s.at(e);
                s.shoot(
                    &out.join(format!("wall_{name}_side.png")),
                    at + DVec3::new(4.4, 1.05, -0.4),
                    at + DVec3::new(0.0, 0.8, -0.4),
                );
                s.shoot(
                    &out.join(format!("wall_{name}_behind.png")),
                    at + DVec3::new(2.0, 1.75, 2.9),
                    at + DVec3::new(0.0, 0.75, -0.3),
                );
            }
        }
        "film" => {
            let e = s.person(DVec3::ZERO, 23, None);
            s.ready();
            s.once = true;
            s.run(0.5);
            // (from s, crouched, speed m/s, way: x left, z ahead)
            let plan = [
                (0.0, false, 0.0, DVec3::Z),
                (0.6, true, 0.0, DVec3::Z),
                (1.6, true, 1.1, DVec3::Z),
                (3.8, true, 2.4, DVec3::Z),
                (5.2, true, 0.8, DVec3::X),
                (6.4, true, 0.0, DVec3::Z),
                (7.0, false, 0.0, DVec3::Z),
            ];
            let frames = (8.0 / STEP) as usize;
            let mut cam = s.at(e);
            for f in 0..frames {
                let t = f as f64 * STEP;
                let &(_, crouched, speed, way) = plan.iter().rev().find(|p| p.0 <= t).unwrap();
                s.crouch(e, crouched);
                let along = DVec3::new(-way.x, 0.0, -way.z);
                s.app.world.get_mut::<Transform>(e).unwrap().translation += along * speed * STEP;
                s.app.update();
                // The camera trails the body a little, alongside.
                cam += (s.at(e) - cam) * 0.08;
                if f % 2 == 0 {
                    s.shoot(
                        &out.join(format!("film_{:03}.png", f / 2)),
                        cam + DVec3::new(4.6, 1.15, -0.6),
                        cam + DVec3::new(0.0, 0.75, -0.6),
                    );
                }
            }
        }
        "gaits" => {
            // (name, speed m/s, way: x left, z ahead)
            let runs = [
                ("walk", 1.1, DVec3::Z),
                ("run", 2.4, DVec3::Z),
                ("back", 0.8, -DVec3::Z),
                ("left", 0.8, DVec3::X),
            ];
            let people: Vec<Entity> = (0..runs.len())
                .map(|i| s.person(DVec3::new(40.0 * i as f64, 0.0, 0.0), 30 + i as u64, None))
                .collect();
            s.ready();
            for &e in &people {
                s.crouch(e, true);
            }
            for f in 0..150 {
                for (&e, (_, speed, way)) in people.iter().zip(&runs) {
                    let along = DVec3::new(-way.x, 0.0, -way.z);
                    s.app.world.get_mut::<Transform>(e).unwrap().translation +=
                        along * *speed * STEP;
                }
                s.app.update();
                if f >= 90 && f % 6 == 0 && f < 90 + 6 * 8 {
                    for (&e, (name, ..)) in people.iter().zip(&runs) {
                        let p = s.at(e);
                        s.shoot(
                            &out.join(format!("gait_{name}_{}.png", (f - 90) / 6)),
                            p + DVec3::new(3.6, 1.0, 0.3),
                            p + DVec3::new(0.0, 0.7, 0.0),
                        );
                    }
                }
            }
        }
        other => eprintln!("unknown mode {other}"),
    }
}
