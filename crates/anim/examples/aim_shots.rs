//! Renders two sword fighters trading aimed strikes, offscreen: a contact sheet of the hit
//! frame at each zone, or a film of six presses at one knee.
//!
//! ```text
//! cargo run -p engine_anim --example aim_shots --release -- <aim|combo6|combo> [out_dir] [width] [height]
//! ```
//!
//! * `aim`: one still per zone (head, neck, arm, knee, the hamstring of a foe turned away, a
//!   stab at a guarding foe's belly), each at the frame the blow lands.
//! * `combo6`: six light presses at the foe's left knee, every other frame (30 fps).
//! * `combo`: the same six presses without a zone (the fixed combo), for comparison.
//!
//! Needs a GPU or a software Vulkan driver (lavapipe). Time runs in fixed 1/60 s steps, so the
//! pictures are the same on every run.

use engine_anim::AnimationPlugin;
use engine_anim::combat::Attacker;
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_render::prelude::*;
use engine_render::renderer::Renderer;
use engine_render::{RenderDisabled, capture_now};
use engine_world::WorldPlugin;
use engine_world::animation::{BodyRequest, StrikeTargets};
use engine_world::combat::{BodyZone, CombatSkills, ItemClass, Loadout};
use engine_world::events::AttackKind;
use engine_world::locomotion::MoveIntent;

const STEP: f64 = 1.0 / 60.0;

struct Stage {
    app: App,
    camera: Entity,
    a: Entity,
    b: Entity,
    parked: Option<Renderer>,
}

/// Two fighters a step apart along X, facing each other (`b` turned away if `away`), both
/// masters with the sword, `b` with a shield only if `shield` (so a blow shows on its mark).
fn stage(width: u32, height: u32, away: bool, shield: bool) -> Stage {
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
        Transform::default().looking_at(DVec3::new(-0.35, -0.62, -0.7), Vec3::Y),
    ));
    w.spawn((
        Transform::default(),
        MeshRenderer::new(
            Shape::Plane {
                size: [400.0, 400.0],
                subdivisions: 1,
            },
            StandardMaterial::hex(0x6b6250).with_roughness(0.95),
        ),
    ));
    let camera = w
        .spawn((
            Camera::perspective(32.0),
            Transform::from_xyz(0.0, 1.2, 4.6).looking_at(DVec3::new(0.0, 0.95, 0.0), Vec3::Y),
        ))
        .id();
    let fighter = |w: &mut World, x: f64, yaw: f32, seed: u64, shield: bool| {
        let t = Transform::from_translation(DVec3::new(x, 0.0, 0.0))
            .with_rotation(Quat::from_rotation_y(yaw.to_radians()));
        w.spawn((
            t,
            GlobalTransform::from(t),
            BodyRequest::human(seed),
            Loadout {
                main: ItemClass::OneHandedBlade,
                off: if shield {
                    ItemClass::Shield
                } else {
                    ItemClass::None
                },
                two_handed: false,
            },
            MoveIntent::default(),
            StrikeTargets::default(),
            CombatSkills {
                weapons: [(ItemClass::OneHandedBlade, 0.9)].into_iter().collect(),
                off_hand: 0.5,
            },
        ))
        .id()
    };
    let a = fighter(w, -0.7, -90.0, 11, true);
    let b = fighter(w, 0.7, if away { -90.0 } else { 90.0 }, 12, shield);
    for (me, foe) in [(a, b), (b, a)] {
        app.world.get_mut::<StrikeTargets>(me).unwrap().target = Some(foe);
    }
    app.update();
    engine_anim::character::finish_loading(&mut app.world);
    // The loadout's props are spawned in an update: let the renderer take them in before
    // it is parked.
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
    let mut s = Stage {
        app,
        camera,
        a,
        b,
        parked,
    };
    for _ in 0..40 {
        s.step();
    }
    s
}

impl Stage {
    /// One frame, the fighters carried by their strikes' steps (as NPC AI's mover does).
    fn step(&mut self) {
        self.app.update();
        for e in [self.a, self.b] {
            let step = self
                .app
                .world
                .get::<Attacker>(e)
                .unwrap()
                .warp_delta
                .as_dvec3();
            self.app.world.get_mut::<Transform>(e).unwrap().translation += step;
        }
    }

    fn attacker(&self) -> &Attacker {
        self.app.world.get::<Attacker>(self.a).unwrap()
    }

    /// Render from `eye` at `target`, both relative to the middle between the fighters.
    fn shoot(&mut self, path: &std::path::Path, eye: Vec3, target: Vec3) {
        let at = |e| self.app.world.get::<Transform>(e).unwrap().translation;
        let mid = (at(self.a) + at(self.b)) * 0.5;
        let t = Transform::from_translation(mid + eye.as_dvec3())
            .looking_at(mid + target.as_dvec3(), Vec3::Y);
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

    /// Press a light strike for a frame.
    fn press(&mut self) {
        self.app.world.get_mut::<MoveIntent>(self.a).unwrap().attack = Some(AttackKind::Melee);
        self.step();
        self.app.world.get_mut::<MoveIntent>(self.a).unwrap().attack = None;
    }
}

/// The side view both fighters fill, a little in front of them.
const SIDE: (Vec3, Vec3) = (Vec3::new(0.35, 1.25, 3.4), Vec3::new(0.0, 0.95, 0.0));

fn main() {
    let mut args = std::env::args().skip(1);
    let mode = args.next().unwrap_or_else(|| "aim".into());
    let out = std::path::PathBuf::from(args.next().unwrap_or_else(|| "aim_shots".into()));
    let width = args.next().and_then(|a| a.parse().ok()).unwrap_or(1280);
    let height = args.next().and_then(|a| a.parse().ok()).unwrap_or(720);
    std::fs::create_dir_all(&out).expect("output directory");
    match mode.as_str() {
        "aim" => {
            // (name, zone, foe turned away, foe guarding, camera)
            let behind = (Vec3::new(2.2, 1.1, 2.6), Vec3::new(0.3, 0.8, 0.0));
            let shots = [
                ("head", BodyZone::Head, false, false, SIDE),
                ("neck", BodyZone::Neck, false, false, SIDE),
                ("arm", BodyZone::UpperArmL, false, false, SIDE),
                ("knee", BodyZone::KneeL, false, false, SIDE),
                ("hamstring_behind", BodyZone::ThighR, true, false, behind),
                ("chest_stab", BodyZone::Belly, false, true, SIDE),
            ];
            for (name, zone, away, guard, (eye, target)) in shots {
                let mut s = stage(width, height, away, guard);
                s.app.world.get_mut::<StrikeTargets>(s.a).unwrap().zone = Some(zone);
                if guard {
                    s.app.world.get_mut::<MoveIntent>(s.b).unwrap().block = true;
                    for _ in 0..20 {
                        s.step();
                    }
                }
                s.press();
                let mut taken = false;
                for _ in 0..90 {
                    s.step();
                    let at = s.attacker();
                    if at.hits + at.blocked + at.deflected > 0 {
                        eprintln!(
                            "{name}: {} (hit {}, blocked {}, deflected {})",
                            at.current.clone().unwrap_or_default(),
                            at.hits,
                            at.blocked,
                            at.deflected
                        );
                        s.shoot(&out.join(format!("{name}.png")), eye, target);
                        taken = true;
                        break;
                    }
                }
                if !taken {
                    eprintln!("{name}: never landed ({:?})", s.attacker().current);
                }
            }
        }
        "combo6" | "combo" => {
            let mut s = stage(width, height, false, true);
            if mode == "combo6" {
                s.app.world.get_mut::<StrikeTargets>(s.a).unwrap().zone = Some(BodyZone::KneeL);
            }
            let (mut thrown, mut frame) = (0, 0);
            let mut last: Option<String> = None;
            let mut pressed = false;
            for k in 0..(9.0 / STEP) as usize {
                let at = s.attacker();
                let ready = at.current.is_none() || at.cancel_open;
                // Press as the last strike gives way, once, and let go.
                pressed = thrown < 6 && ready && !pressed;
                if pressed {
                    s.press();
                } else {
                    s.step();
                }
                let at = s.attacker();
                if at.current.is_some() && at.current != last {
                    thrown += 1;
                    eprintln!("{:5.2}s {}", k as f64 * STEP, at.current.clone().unwrap());
                    last = at.current.clone();
                }
                if at.current.is_none() {
                    last = None;
                }
                if k % 2 == 0 {
                    s.shoot(&out.join(format!("{frame:05}.png")), SIDE.0, SIDE.1);
                    frame += 1;
                }
                if thrown >= 6 && s.attacker().current.is_none() {
                    break;
                }
            }
        }
        "knee_strip" => {
            // Every third frame of one cut at the knee, from the press to the stance.
            let mut s = stage(width, height, false, false);
            s.app.world.get_mut::<StrikeTargets>(s.a).unwrap().zone = Some(BodyZone::KneeL);
            s.press();
            for k in 0..45 {
                s.step();
                if k % 3 == 0 {
                    let at = s.attacker();
                    eprintln!("{k}: {:?} hits {}", at.current, at.hits);
                    s.shoot(&out.join(format!("k{k:02}.png")), SIDE.0, SIDE.1);
                }
            }
        }
        "states" => {
            // The foe going down without physics and getting up; the near fighter kneeling.
            let mut s = stage(width, height, false, true);
            s.app
                .world
                .entity_mut(s.b)
                .insert(engine_world::animation::Ragdoll { weight: 1.0 });
            s.app
                .world
                .get_mut::<engine_world::animation::AnimationParams>(s.a)
                .unwrap()
                .set("kneel", 1.0);
            let near = (Vec3::new(0.2, 1.2, 3.0), Vec3::new(0.0, 0.5, 0.0));
            for k in 0..=330 {
                if k == 200 {
                    s.app
                        .world
                        .get_mut::<engine_world::animation::Ragdoll>(s.b)
                        .unwrap()
                        .weight = 0.0;
                }
                if [12, 30, 50, 100, 230, 260, 300].contains(&k) {
                    s.shoot(&out.join(format!("s{k:03}.png")), near.0, near.1);
                }
                s.step();
            }
        }
        "bow" => {
            // An archer drawing on cue, holding at full draw, loosing on cue; the foe out of
            // the way.
            let mut s = stage(width, height, false, false);
            *s.app.world.get_mut::<Loadout>(s.a).unwrap() = Loadout {
                main: ItemClass::Bow,
                off: ItemClass::None,
                two_handed: true,
            };
            s.app.world.get_mut::<Transform>(s.b).unwrap().translation.x = 4.7;
            for _ in 0..30 {
                s.step();
            }
            let cue = |s: &mut Stage, name: &str| {
                let entity = s.a;
                s.app
                    .world
                    .write_message(engine_world::animation::AnimationTrigger {
                        entity,
                        name: name.into(),
                    });
            };
            let side = (Vec3::new(-2.7, 1.4, 3.2), Vec3::new(-2.7, 1.25, 0.0));
            let front = (Vec3::new(-0.9, 1.6, 1.7), Vec3::new(-2.7, 1.35, 0.0));
            cue(&mut s, "draw");
            for k in 0..=300 {
                if k == 240 {
                    cue(&mut s, "loose");
                }
                if [20, 60, 200, 243, 262].contains(&k) {
                    s.shoot(&out.join(format!("b{k:03}_side.png")), side.0, side.1);
                    s.shoot(&out.join(format!("b{k:03}_front.png")), front.0, front.1);
                }
                s.step();
            }
        }
        "rifle" => {
            // A carbine at the low ready, shouldered on `aim`, a shot, a magazine change.
            let mut s = stage(width, height, false, false);
            *s.app.world.get_mut::<Loadout>(s.a).unwrap() = Loadout {
                main: ItemClass::Firearm,
                off: ItemClass::None,
                two_handed: true,
            };
            s.app.world.get_mut::<Transform>(s.b).unwrap().translation.x = 4.7;
            for _ in 0..30 {
                s.step();
            }
            let cue = |s: &mut Stage, name: &str| {
                let entity = s.a;
                s.app
                    .world
                    .write_message(engine_world::animation::AnimationTrigger {
                        entity,
                        name: name.into(),
                    });
            };
            let side = (Vec3::new(-2.7, 1.4, 3.2), Vec3::new(-2.7, 1.2, 0.0));
            let front = (Vec3::new(-0.9, 1.6, 1.7), Vec3::new(-2.7, 1.3, 0.0));
            for k in 0..=330 {
                match k {
                    30 => s
                        .app
                        .world
                        .get_mut::<engine_world::animation::AnimationParams>(s.a)
                        .unwrap()
                        .set("aim", 1.0),
                    90 => cue(&mut s, "fire"),
                    120 => cue(&mut s, "reload"),
                    _ => {}
                }
                if [20, 80, 92, 142, 160, 178, 213, 220, 300].contains(&k) {
                    // The first view of a frame can show a mesh where it was last drawn: it
                    // is taken again after the second.
                    s.shoot(&out.join(format!("r{k:03}_side.png")), side.0, side.1);
                    s.shoot(&out.join(format!("r{k:03}_front.png")), front.0, front.1);
                    s.shoot(&out.join(format!("r{k:03}_side.png")), side.0, side.1);
                }
                s.step();
            }
        }
        _ => eprintln!("modes: aim, combo6, combo, knee_strip, states, bow, rifle"),
    }
}
