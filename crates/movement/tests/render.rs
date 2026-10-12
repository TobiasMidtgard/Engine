//! Headless screenshot of the duel in reflex slow motion: the demo renders, and the slow-motion
//! screen effect (vignette, grade) is drawn on top. Needs a GPU or a software Vulkan driver
//! (lavapipe); passes with a note when neither is available.

use engine_core::math::{DVec3, Vec3};
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_movement::demo::{self, DuelDirector, Round};
use engine_movement::{MovementPlugin, PerceptionStatus, SlowMotionFx};
use engine_render::prelude::*;
use engine_render::{RenderDisabled, capture_now};
use engine_world::WorldPlugin;

fn mean_rgb(img: &image::RgbaImage, x0: u32, y0: u32, w: u32, h: u32) -> [f32; 3] {
    let mut sum = [0.0f32; 3];
    for y in y0..y0 + h {
        for x in x0..x0 + w {
            let p = img.get_pixel(x, y);
            for c in 0..3 {
                sum[c] += p[c] as f32;
            }
        }
    }
    sum.map(|s| s / (w * h) as f32)
}

#[test]
fn renders_the_duel_with_the_slow_motion_effect() {
    let (width, height) = (320, 180);
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = width;
        w.height = height;
    }
    app.add_plugins(RenderPlugin)
        .add_plugins(WorldPlugin)
        .add_plugins(MovementPlugin);
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 60.0,
        });
    // Simulate without drawing; draw only the frames we check.
    app.world
        .insert_resource(RenderDisabled("not drawing yet".into()));
    app.world.spawn((
        DirectionalLight::default(),
        Transform::default().looking_at(DVec3::new(-0.6, -0.5, 0.4), Vec3::Y),
    ));
    app.world.spawn((
        Transform::default(),
        MeshRenderer::new(
            Shape::Plane {
                size: [200.0, 200.0],
                subdivisions: 1,
            },
            StandardMaterial::hex(0x3a3e44).with_roughness(0.9),
        ),
    ));
    app.world.spawn((
        Camera::perspective(50.0).with_exposure(Exposure::Manual { ev100: 14.0 }),
        Transform::from_xyz(-6.0, 3.0, -6.0).looking_at(DVec3::new(0.0, 1.0, -12.0), Vec3::Y),
    ));
    let mut queue = bevy_ecs::world::CommandQueue::default();
    let mut commands = Commands::new(&mut queue, &app.world);
    demo::spawn_demo(&mut commands, DVec3::ZERO);
    queue.apply(&mut app.world);
    {
        let mut q = app.world.query::<&mut DuelDirector>();
        let mut d = q.single_mut(&mut app.world).unwrap();
        d.round = Round::Focus;
        d.only = Some(Round::Focus);
    }

    // Run until the elite attacks and the world slows.
    let mut focused = false;
    for _ in 0..240 {
        app.update();
        if app.world.resource::<PerceptionStatus>().focused
            && app.world.resource::<Time>().scale() < 0.3
        {
            focused = true;
            break;
        }
    }
    assert!(focused, "reflex slow motion never engaged");

    app.world.remove_resource::<RenderDisabled>();
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"));
    let with_fx = dir.join("movement_focus.png");
    if let Err(e) = capture_now(&mut app.world, &with_fx) {
        eprintln!("skipping: no GPU ({e})");
        return;
    }
    app.world.resource_mut::<SlowMotionFx>().enabled = false;
    let without_fx = dir.join("movement_focus_no_fx.png");
    capture_now(&mut app.world, &without_fx).expect("screenshot");

    let a = image::open(&with_fx).expect("png").to_rgba8();
    let b = image::open(&without_fx).expect("png").to_rgba8();
    let mut distinct = std::collections::HashSet::new();
    for p in b.pixels() {
        distinct.insert([p[0] / 16, p[1] / 16, p[2] / 16]);
    }
    assert!(
        distinct.len() > 30,
        "flat image ({} colors)",
        distinct.len()
    );
    // The vignette darkens the corners; the middle stays about the same.
    let corner = |img: &image::RgbaImage| mean_rgb(img, 0, 0, 40, 30).iter().sum::<f32>();
    let middle = |img: &image::RgbaImage| {
        mean_rgb(img, width / 2 - 20, height / 2 - 15, 40, 30)
            .iter()
            .sum::<f32>()
    };
    assert!(
        corner(&a) < corner(&b) * 0.9,
        "corners {} with the effect, {} without",
        corner(&a),
        corner(&b)
    );
    assert!(
        (middle(&a) - middle(&b)).abs() < middle(&b) * 0.2 + 10.0,
        "middle {} vs {}",
        middle(&a),
        middle(&b)
    );
    eprintln!("saved {} and {}", with_fx.display(), without_fx.display());
}
