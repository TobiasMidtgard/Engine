//! Renders close-ups of the animation paddock offscreen, one PNG per feature.
//!
//! ```text
//! cargo run -p engine_anim --example anim_gallery --release -- [out_dir] [width] [height]
//! ```
//!
//! Needs a GPU or a software Vulkan driver (lavapipe). Time runs in fixed 1/60 s steps, so the
//! pictures are the same on every run.

use engine_anim::demo::{DemoEntities, spawn_demo_world};
use engine_anim::{AnimationPlugin, Animator, MuscleRig};
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_render::prelude::*;
use engine_render::renderer::Renderer;
use engine_render::{DebugView, RenderDisabled, capture_now};
use engine_world::WorldPlugin;
use engine_world::animation::AnimationParams;

/// When a shot is taken.
enum When {
    /// At this many seconds after the demo starts.
    At(f64),
    /// The first frame (after `after` seconds) where the condition holds.
    First {
        after: f64,
        test: fn(&mut World, &DemoEntities) -> bool,
    },
}

struct Shot {
    name: &'static str,
    when: When,
    subject: fn(&DemoEntities) -> Entity,
    /// Camera position in the subject's frame (the subject faces −Z, its right is +X).
    eye: Vec3,
    /// Look-at point in the subject's frame.
    target: Vec3,
    fov_deg: f32,
}

fn param(world: &World, e: Entity, name: &str) -> f32 {
    world
        .get::<AnimationParams>(e)
        .map(|p| p.get(name))
        .unwrap_or(0.0)
}

fn position(world: &World, e: Entity) -> DVec3 {
    world
        .get::<GlobalTransform>(e)
        .map(|g| g.translation())
        .unwrap_or_default()
}

fn shots() -> Vec<Shot> {
    vec![
        Shot {
            name: "01_reach_sleeves_bunch",
            when: When::At(1.2),
            subject: |d| d.reacher,
            eye: Vec3::new(1.4, 1.45, -1.9),
            target: Vec3::new(0.1, 1.2, -0.25),
            fov_deg: 38.0,
        },
        Shot {
            name: "02_hammer_lift",
            when: When::At(1.2),
            subject: |d| d.athlete,
            eye: Vec3::new(3.4, 1.5, -1.4),
            target: Vec3::new(0.0, 1.2, -0.3),
            fov_deg: 42.0,
        },
        Shot {
            name: "03_hammer_strike",
            when: When::At(1.57),
            subject: |d| d.athlete,
            eye: Vec3::new(2.9, 1.3, -2.3),
            target: Vec3::new(0.0, 0.9, -0.5),
            fov_deg: 42.0,
        },
        Shot {
            name: "04_reach_back_stretch",
            when: When::At(2.06),
            subject: |d| d.reacher,
            eye: Vec3::new(0.9, 1.65, 1.8),
            target: Vec3::new(0.0, 1.3, -0.1),
            fov_deg: 38.0,
        },
        Shot {
            name: "05_flex_double_biceps",
            when: When::At(2.5),
            subject: |d| d.lifter,
            eye: Vec3::new(0.35, 1.45, -2.4),
            target: Vec3::new(0.0, 1.3, 0.0),
            fov_deg: 38.0,
        },
        Shot {
            name: "06_flex_back",
            when: When::At(3.3),
            subject: |d| d.lifter,
            eye: Vec3::new(-0.8, 1.55, 2.2),
            target: Vec3::new(0.0, 1.3, 0.0),
            fov_deg: 38.0,
        },
        Shot {
            name: "07_relaxed",
            when: When::At(3.5),
            subject: |d| d.athlete,
            eye: Vec3::new(1.6, 1.4, -3.2),
            target: Vec3::new(0.0, 1.0, 0.0),
            fov_deg: 40.0,
        },
        Shot {
            name: "08_block",
            when: When::At(4.8),
            subject: |d| d.athlete,
            eye: Vec3::new(1.2, 1.5, -2.6),
            target: Vec3::new(0.0, 1.2, 0.0),
            fov_deg: 38.0,
        },
        Shot {
            name: "09_walker_ramp",
            when: When::First {
                after: 0.0,
                test: |w, d| position(w, d.walker).y > 0.35,
            },
            subject: |d| d.walker,
            eye: Vec3::new(3.2, 1.2, -1.2),
            target: Vec3::new(0.0, 0.7, 0.0),
            fov_deg: 45.0,
        },
        Shot {
            name: "10_walker_stairs",
            when: When::First {
                after: 0.0,
                test: |w, d| {
                    let p = position(w, d.walker);
                    p.x < -12.3 && p.y > 0.15 && p.y < 0.6
                },
            },
            subject: |d| d.walker,
            eye: Vec3::new(3.0, 1.0, -0.6),
            target: Vec3::new(0.0, 0.6, 0.0),
            fov_deg: 45.0,
        },
        Shot {
            name: "11_walker_runs",
            when: When::First {
                after: 0.0,
                test: |w, d| param(w, d.walker, "speed") > 3.5,
            },
            subject: |d| d.walker,
            eye: Vec3::new(3.0, 1.3, -2.0),
            target: Vec3::new(0.0, 0.95, 0.0),
            fov_deg: 45.0,
        },
        Shot {
            name: "12_horse_trot",
            when: When::First {
                after: 0.0,
                test: |w, d| param(w, d.horse, "speed") > 3.3,
            },
            subject: |d| d.horse,
            eye: Vec3::new(4.5, 1.7, -2.0),
            target: Vec3::new(0.0, 1.2, 0.0),
            fov_deg: 45.0,
        },
        Shot {
            name: "13_horse_gallop",
            when: When::First {
                after: 0.0,
                test: |w, d| param(w, d.horse, "speed") > 10.0,
            },
            subject: |d| d.horse,
            eye: Vec3::new(5.0, 1.6, -1.5),
            target: Vec3::new(0.0, 1.2, 0.3),
            fov_deg: 45.0,
        },
        Shot {
            name: "14_horse_flank_muscles",
            when: When::First {
                after: 0.0,
                test: |w, d| {
                    param(w, d.horse, "speed") > 10.0
                        && w.get::<MuscleRig>(d.horse)
                            .is_some_and(|m| m.activations().iter().any(|(_, a)| *a > 0.6))
                },
            },
            subject: |d| d.horse,
            eye: Vec3::new(3.0, 1.5, 1.6),
            target: Vec3::new(0.0, 1.2, 0.5),
            fov_deg: 40.0,
        },
        Shot {
            name: "15_spider",
            when: When::At(6.0),
            subject: |d| d.spider,
            eye: Vec3::new(1.9, 1.0, -1.7),
            target: Vec3::new(0.0, 0.3, 0.05),
            fov_deg: 45.0,
        },
        Shot {
            name: "23_spider_head",
            when: When::At(3.0),
            subject: |d| d.spider,
            eye: Vec3::new(0.45, 0.5, -0.85),
            target: Vec3::new(0.0, 0.3, -0.25),
            fov_deg: 45.0,
        },
        Shot {
            name: "16_car_bump",
            when: When::First {
                after: 1.0,
                test: |w, d| position(w, d.car).y > 0.04,
            },
            subject: |d| d.car,
            eye: Vec3::new(4.5, 1.4, -4.0),
            target: Vec3::new(0.0, 0.6, 0.0),
            fov_deg: 45.0,
        },
        Shot {
            name: "17_car_corner",
            when: When::First {
                after: 2.0,
                test: |w, d| param(w, d.car, "turn").abs() > 0.6,
            },
            subject: |d| d.car,
            eye: Vec3::new(-3.0, 2.0, -6.0),
            target: Vec3::new(0.0, 0.6, 0.0),
            fov_deg: 45.0,
        },
        Shot {
            name: "19_dog_head",
            when: When::At(1.5),
            subject: |d| d.dog,
            eye: Vec3::new(0.62, 0.78, -1.05),
            target: Vec3::new(0.0, 0.64, -0.45),
            fov_deg: 35.0,
        },
        Shot {
            name: "20_dog_coat",
            when: When::At(2.0),
            subject: |d| d.dog,
            eye: Vec3::new(0.62, 0.62, 0.12),
            target: Vec3::new(0.0, 0.45, -0.08),
            fov_deg: 32.0,
        },
        Shot {
            name: "21_dog_trot",
            when: When::First {
                after: 0.5,
                test: |w, d| param(w, d.dog, "gait.stage") == 1.0 && param(w, d.dog, "speed") > 2.3,
            },
            subject: |d| d.dog,
            eye: Vec3::new(2.0, 0.65, -0.6),
            target: Vec3::new(0.0, 0.38, 0.0),
            fov_deg: 38.0,
        },
        Shot {
            name: "22_dog_gallop",
            when: When::First {
                after: 0.5,
                test: |w, d| param(w, d.dog, "gait.stage") == 3.0 && param(w, d.dog, "speed") > 8.0,
            },
            subject: |d| d.dog,
            eye: Vec3::new(2.4, 0.6, -1.0),
            target: Vec3::new(0.0, 0.38, 0.0),
            fov_deg: 38.0,
        },
        Shot {
            name: "18_overview",
            when: When::At(12.0),
            subject: |d| d.athlete,
            eye: Vec3::new(3.0, 16.0, -27.5),
            target: Vec3::new(0.0, 0.0, 12.0),
            fov_deg: 55.0,
        },
    ]
}

fn sequence(spec: &str) -> Vec<Shot> {
    let parts: Vec<&str> = spec.split(':').collect();
    let subject: fn(&DemoEntities) -> Entity = match parts[0] {
        "athlete" => |d| d.athlete,
        "lifter" => |d| d.lifter,
        "reacher" => |d| d.reacher,
        "walker" => |d| d.walker,
        "drill" => |d| d.drill,
        "sparring" => |d| d.sparring,
        "horse" => |d| d.horse,
        "spider" => |d| d.spider,
        "dog" => |d| d.dog,
        "stuntman" => |d| d.stuntman,
        "runner" => |d| d.runner,
        "coach" => |d| d.coach,
        "spectator" => |d| d.spectators[0],
        // The camera fixed in the paddock's frame.
        "world" => |_| Entity::PLACEHOLDER,
        _ => |d| d.car,
    };
    let num = |i: usize, default: f64| parts.get(i).and_then(|p| p.parse().ok()).unwrap_or(default);
    let (start, end, step) = (num(1, 0.5), num(2, 3.0), num(3, 0.1));
    let eye = parts
        .get(4)
        .map(|p| {
            let v: Vec<f32> = p.split(',').filter_map(|x| x.parse().ok()).collect();
            Vec3::new(v[0], v[1], v[2])
        })
        .unwrap_or(Vec3::new(-3.6, 1.3, -0.6));
    let target = parts
        .get(5)
        .map(|p| {
            let v: Vec<f32> = p.split(',').filter_map(|x| x.parse().ok()).collect();
            Vec3::new(v[0], v[1], v[2])
        })
        .unwrap_or(Vec3::new(0.0, (eye.y * 0.85).min(1.3), 0.0));
    let mut out = Vec::new();
    let mut t = start;
    let mut k = 0;
    while t <= end + 1e-6 {
        out.push(Shot {
            name: Box::leak(format!("seq_{k:03}").into_boxed_str()),
            when: When::At(t),
            subject,
            eye,
            target,
            fov_deg: 40.0,
        });
        t += step;
        k += 1;
    }
    out
}

fn main() {
    let mut args = std::env::args().skip(1);
    let out = std::path::PathBuf::from(args.next().unwrap_or_else(|| "anim_gallery".into()));
    let width = args.next().and_then(|a| a.parse().ok()).unwrap_or(1280);
    let height = args.next().and_then(|a| a.parse().ok()).unwrap_or(720);
    std::fs::create_dir_all(&out).expect("output directory");

    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = width;
        w.height = height;
    }
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 60.0,
        });
    app.add_plugins((WorldPlugin, RenderPlugin, AnimationPlugin));
    if let Ok(view) = std::env::var("GALLERY_VIEW") {
        app.world.resource_mut::<RenderSettings>().debug_view = match view.as_str() {
            "normals" => DebugView::Normals,
            "cascades" => DebugView::Cascades,
            _ => DebugView::SkyVisibility,
        };
    }
    let camera = {
        let w = &mut app.world;
        w.spawn((
            DirectionalLight {
                illuminance: 90_000.0,
                ..Default::default()
            },
            Transform::default().looking_at(DVec3::new(-0.45, -0.62, -0.62), Vec3::Y),
        ));
        w.spawn((
            Transform::default(),
            MeshRenderer::new(
                Shape::Plane {
                    size: [400.0, 400.0],
                    subdivisions: 1,
                },
                StandardMaterial::hex(0x56723c).with_roughness(0.95),
            ),
        ));
        w.spawn((
            Camera::perspective(45.0),
            Transform::from_xyz(0.0, 3.0, 20.0).looking_at(DVec3::ZERO, Vec3::Y),
        ))
        .id()
    };
    let demo = spawn_demo_world(&mut app.world, DVec3::ZERO);
    engine_anim::character::finish_loading(&mut app.world);
    app.update();
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("no GPU: {}", d.0);
        std::process::exit(1);
    }

    let mut pending = match std::env::var("GALLERY_SEQUENCE") {
        // A strip of one subject over time, for tuning a motion:
        // GALLERY_SEQUENCE=athlete:0.6:2.6:0.1[:eye x,y,z[:target x,y,z]]
        Ok(spec) => sequence(&spec),
        Err(_) => shots(),
    };
    // Play one graph state on the subject at a time, for a motion the demo only reaches
    // through physics: GALLERY_TRIGGER=get_up_back:1.0 (the stuntman stops his routine).
    let mut trigger = std::env::var("GALLERY_TRIGGER").ok().and_then(|spec| {
        let (name, at) = spec.split_once(':')?;
        Some((name.to_string(), at.parse::<f64>().ok()?))
    });
    let step = 1.0 / 60.0;
    let mut time = step;
    let end = 40.0;
    // Only shots are rendered: park the renderer between them (software rendering is slow).
    let mut parked = app.world.remove_resource::<Renderer>();
    app.world
        .insert_resource(RenderDisabled("parked between shots".into()));
    while !pending.is_empty() && time < end {
        if let Some((name, _)) = trigger.take_if(|(_, at)| time >= *at) {
            let e = demo.stuntman;
            app.world
                .entity_mut(e)
                .remove::<engine_anim::stunt::Stunt>();
            if let Some(mut animator) = app.world.get_mut::<Animator>(e) {
                animator.trigger(&name);
            }
        }
        app.update();
        time += step;
        let mut k = 0;
        while k < pending.len() {
            let due = match pending[k].when {
                When::At(t) => time >= t,
                When::First { after, test } => time >= after && test(&mut app.world, &demo),
            };
            if !due {
                k += 1;
                continue;
            }
            let shot = pending.remove(k);
            let subject = (shot.subject)(&demo);
            let (pos, rot) = app
                .world
                .get::<GlobalTransform>(subject)
                .map(|g| (g.translation(), g.rotation()))
                .unwrap_or((DVec3::ZERO, Quat::IDENTITY));
            if std::env::var("GALLERY_DEBUG").is_ok() {
                let (y, x, z) = rot.to_euler(EulerRot::YXZ);
                eprintln!(
                    "{}: pos {pos:.2} yaw {:.1} pitch {:.1} roll {:.1}",
                    shot.name,
                    y.to_degrees(),
                    x.to_degrees(),
                    z.to_degrees()
                );
            }
            let eye = pos + (rot * shot.eye).as_dvec3();
            let target = pos + (rot * shot.target).as_dvec3();
            let t = Transform::from_translation(eye).looking_at(target, Vec3::Y);
            if let Some(mut cam) = app.world.get_mut::<Camera>(camera) {
                *cam = Camera::perspective(shot.fov_deg);
            }
            app.world
                .entity_mut(camera)
                .insert((t, GlobalTransform::from(t)));
            let path = out.join(format!("{}.png", shot.name));
            app.world.remove_resource::<RenderDisabled>();
            if let Some(r) = parked.take() {
                app.world.insert_resource(r);
            }
            match capture_now(&mut app.world, &path) {
                Ok(_) => println!("{:6.2}s  {}", time, path.display()),
                Err(e) => eprintln!("{}: {e}", shot.name),
            }
            parked = app.world.remove_resource::<Renderer>();
            app.world
                .insert_resource(RenderDisabled("parked between shots".into()));
        }
    }
    for shot in pending {
        eprintln!("never taken: {}", shot.name);
    }
}
