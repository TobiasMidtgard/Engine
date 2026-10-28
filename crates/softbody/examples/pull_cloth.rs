//! Pulling cloth by hand with the use key (E): a stand-in player walks up to the covered
//! statue in the yard, takes hold of the silk sheet's hem and backs away, dragging it off.
//! Saves one PNG per moment. Software Vulkan (lavapipe) is enough.
//!
//! ```sh
//! cargo run -p engine_softbody --example pull_cloth --release -- [out_dir] [WIDTHxHEIGHT] [shot,...]
//! ```

use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, capture_now};
use engine_world::interact::Interact;

/// The way the statue faces.
const FACING: Vec3 = Vec3::new(0.5, 0.0, -0.87);
/// Seconds into the hoist's cycle when the sheet has settled over the statue.
const COVERED: f32 = 10.0;
/// Where the player stands to take hold (meters in front of the statue), and how far it
/// backs away, at walking pace.
const GRAB_AT: f64 = 1.3;
const BACK_OFF: f64 = 2.6;
const WALK_SECS: f32 = 2.6;

/// Seconds into the cycle, and the camera: distance out in front, to the statue's right,
/// height; and the height it looks at.
const SHOTS: &[(&str, f32, [f64; 3], f64)] = &[
    ("covered", COVERED, [4.6, 3.0, 2.0], 1.6),
    ("taking_hold", COVERED + 0.6, [4.6, 3.0, 2.0], 1.6),
    (
        "pulling",
        COVERED + 0.6 + 0.6 * WALK_SECS,
        [5.2, 3.4, 2.1],
        1.5,
    ),
    ("pulled", COVERED + 0.9 + WALK_SECS, [5.6, 3.6, 2.2], 1.4),
];

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let out =
        std::path::PathBuf::from(args.get(1).map_or("screenshots/pull_cloth", String::as_str));
    let (width, height) = args
        .get(2)
        .and_then(|s| s.split_once('x'))
        .and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)))
        .unwrap_or((1280u32, 720u32));
    let only: Option<Vec<String>> = args
        .get(3)
        .map(|s| s.split(',').map(str::to_string).collect());
    let wanted = |name: &str| only.as_ref().is_none_or(|o| o.iter().any(|n| n == name));
    let last = SHOTS
        .iter()
        .rposition(|(name, ..)| wanted(name))
        .expect("no such shot");
    engine_core::log::init("info,wgpu=warn,naga=warn");

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
            step_secs: 1.0 / 30.0,
        });
    app.add_plugins((
        RenderPlugin,
        engine_world::WorldPlugin,
        engine_softbody::SoftBodyPlugin,
    ));
    // Bare earth around the plinth, meadow beyond.
    app.world.resource_mut::<Atmosphere>().ground_color = [0.11, 0.12, 0.07];
    // The statue's body is built before the first frame.
    app.world
        .resource_mut::<engine_softbody::demo::skin::BodyLibrary>()
        .blocking = true;
    // Afternoon sun over the statue's right shoulder, as at the showcase's tour stop.
    let sun = (FACING * 0.55 + Vec3::new(-FACING.z, 0.0, FACING.x) * 0.45 + Vec3::Y * 0.7)
        .normalize()
        .as_dvec3();
    app.world.spawn((
        DirectionalLight {
            illuminance: 100_000.0,
            ..Default::default()
        },
        Transform::default().looking_at(-sun, Vec3::Y),
    ));
    let camera = app
        .world
        .spawn((
            Camera {
                exposure: Exposure::Manual { ev100: 14.6 },
                ..Camera::perspective(50.0)
            },
            Transform::from_xyz(0.0, 3.0, 8.0).looking_at(DVec3::ZERO, Vec3::Y),
        ))
        .id();
    app.add_systems(Startup, |mut commands: Commands| {
        engine_softbody::demo::spawn_review_ground(&mut commands, DVec3::ZERO);
        engine_softbody::demo::statue::spawn_unveiling(&mut commands, DVec3::ZERO, FACING);
    });

    std::fs::create_dir_all(&out).expect("output directory");
    let front = FACING.as_dvec3();
    let right = DVec3::new(-front.z, 0.0, front.x);
    // The stand-in player: a grey mannequin, its position at its feet, facing the statue.
    let start = front * (GRAB_AT + 4.0);
    let grab_spot = front * GRAB_AT;
    let face = Quat::from_rotation_arc(Vec3::NEG_Z, -FACING.normalize());
    let player = app
        .world
        .spawn((
            Name::new("Player stand-in"),
            Transform::from_translation(start).with_rotation(face),
        ))
        .id();
    app.world.spawn((
        Transform::from_xyz(0.0, 0.875, 0.0),
        MeshRenderer::new(
            Shape::Capsule {
                radius: 0.22,
                height: 1.75,
                segments: 16,
            },
            StandardMaterial::hex(0x8a8f96).with_roughness(0.7),
        ),
        ChildOf(player),
    ));
    let mut grabbed = false;
    let mut elapsed = 0.0f32;
    for (i, (name, secs, [ahead, side, up], look)) in SHOTS.iter().enumerate().take(last + 1) {
        if !wanted(name) {
            continue;
        }
        while elapsed < *secs {
            // Walk up before the grab, back away after it.
            let at = if elapsed < COVERED {
                start.lerp(
                    grab_spot,
                    f64::from((elapsed / COVERED * 4.0 - 3.0).clamp(0.0, 1.0)),
                )
            } else {
                let k = ((elapsed - COVERED - 0.3) / WALK_SECS).clamp(0.0, 1.0);
                grab_spot + front * BACK_OFF * f64::from(k * k * (3.0 - 2.0 * k))
            };
            app.world.get_mut::<Transform>(player).unwrap().translation = at;
            if !grabbed && elapsed >= COVERED {
                let sheet = {
                    let mut q = app.world.query::<(Entity, &Name)>();
                    q.iter(&app.world)
                        .find(|(_, n)| n.as_str() == "Unveiling sheet")
                        .map(|(e, _)| e)
                        .expect("the sheet")
                };
                app.world.write_message(Interact {
                    actor: player,
                    target: sheet,
                    from: at,
                });
                grabbed = true;
            }
            app.update();
            elapsed += 1.0 / 30.0;
        }
        if let Some(d) = app.world.get_resource::<RenderDisabled>() {
            eprintln!("no GPU ({}); nothing to capture", d.0);
            return;
        }
        let eye = front * *ahead + right * *side + DVec3::Y * *up;
        let t =
            Transform::from_translation(eye).looking_at(front * 1.2 + DVec3::Y * *look, Vec3::Y);
        *app.world.get_mut::<Transform>(camera).unwrap() = t;
        *app.world.get_mut::<GlobalTransform>(camera).unwrap() = GlobalTransform::from(t);
        let path = out.join(format!("{:02}_{name}.png", i + 1));
        match capture_now(&mut app.world, &path) {
            Ok(_) => println!("{} ({elapsed:.1} s)", path.display()),
            Err(e) => eprintln!("{name}: {e}"),
        }
    }
    let d = app.world.resource::<Diagnostics>();
    println!(
        "particles: {:?}, step: {:?} ms",
        d.get("softbody.particles"),
        d.get("softbody.step_ms"),
    );
}
