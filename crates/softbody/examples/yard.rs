//! Renders the soft-body yard offscreen from a series of close viewpoints as the demo plays,
//! and saves one PNG per shot. Software Vulkan (lavapipe) is enough.
//!
//! ```sh
//! cargo run -p engine_softbody --example yard --release -- [out_dir] [WIDTHxHEIGHT] [shot,shot]
//! ```

use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, capture_now};

/// One screenshot: when (demo seconds), from where, looking at what (relative to the yard, or
/// in the named entity's own frame when `follow` is set; characters face −Z).
struct Shot {
    name: &'static str,
    secs: f32,
    eye: [f64; 3],
    target: [f64; 3],
    follow: Option<&'static str>,
}

const SHOTS: &[Shot] = &[
    Shot {
        name: "overview",
        secs: 3.0,
        eye: [2.0, 15.0, 36.0],
        target: [4.0, 2.0, -4.0],
        follow: None,
    },
    Shot {
        name: "flags",
        secs: 3.5,
        eye: [-7.0, 6.2, -4.5],
        target: [-7.0, 7.2, -18.0],
        follow: None,
    },
    Shot {
        name: "jelly",
        secs: 4.4,
        eye: [1.5, 3.2, 7.5],
        target: [0.5, 1.8, -1.0],
        follow: None,
    },
    Shot {
        name: "clay_press",
        secs: 5.2,
        eye: [9.0, 2.6, 6.5],
        target: [6.5, 1.5, 2.5],
        follow: None,
    },
    Shot {
        name: "mannequin",
        secs: 6.0,
        eye: [1.7, 1.8, 2.5],
        target: [0.0, 1.2, 0.0],
        follow: Some("Mannequin"),
    },
    Shot {
        name: "mannequin_front",
        secs: 6.2,
        eye: [0.9, 1.45, -2.3],
        target: [0.0, 1.05, 0.0],
        follow: Some("Mannequin"),
    },
    Shot {
        name: "mannequin_face",
        secs: 6.35,
        eye: [0.3, 1.75, -0.8],
        target: [0.0, 1.66, 0.0],
        follow: Some("Mannequin"),
    },
    Shot {
        name: "dog",
        secs: 6.6,
        eye: [1.9, 0.9, 0.9],
        target: [0.0, 0.5, 0.05],
        follow: Some("Dog"),
    },
    Shot {
        name: "dog_front",
        secs: 6.75,
        eye: [0.9, 0.75, -1.4],
        target: [0.0, 0.5, -0.1],
        follow: Some("Dog"),
    },
    Shot {
        name: "dog_legs",
        secs: 6.76,
        eye: [0.45, 0.3, -0.75],
        target: [0.05, 0.17, -0.22],
        follow: Some("Dog"),
    },
    Shot {
        name: "dog_head",
        secs: 6.85,
        eye: [0.45, 0.85, -1.0],
        target: [0.0, 0.74, -0.5],
        follow: Some("Dog"),
    },
    Shot {
        name: "cut_bridge",
        secs: 7.0,
        eye: [-2.0, 5.0, -2.0],
        target: [-2.0, 2.8, -11.0],
        follow: None,
    },
    Shot {
        name: "bridge_walker",
        secs: 8.0,
        eye: [8.0, 6.5, 19.0],
        target: [11.0, 2.5, 10.0],
        follow: None,
    },
    Shot {
        name: "bead_curtain",
        secs: 9.0,
        eye: [0.5, 2.2, 15.0],
        target: [-3.0, 1.3, 9.0],
        follow: None,
    },
    Shot {
        name: "swing",
        secs: 9.5,
        eye: [19.5, 2.5, 9.5],
        target: [15.0, 2.0, 4.0],
        follow: None,
    },
    Shot {
        name: "flag_field",
        secs: 10.0,
        eye: [15.0, 4.5, 2.0],
        target: [24.0, 1.5, -10.0],
        follow: None,
    },
    Shot {
        name: "slow_motion",
        secs: 10.5,
        eye: [-2.2, 1.6, 1.2],
        target: [0.0, 1.2, 0.0],
        follow: Some("Slow-motion mannequin"),
    },
    Shot {
        name: "blanket",
        secs: 7.5,
        eye: [-2.6, 1.7, 5.8],
        target: [-4.4, 0.4, 3.0],
        follow: None,
    },
    Shot {
        name: "blanket_falling",
        secs: 9.35,
        eye: [-1.6, 2.4, 7.2],
        target: [-4.3, 1.0, 3.0],
        follow: None,
    },
    Shot {
        name: "curtains",
        secs: 13.0,
        eye: [-12.0, 3.4, 15.0],
        target: [-12.0, 3.2, 6.0],
        follow: None,
    },
    // Prop close-ups.
    Shot {
        name: "gun",
        secs: 13.3,
        eye: [-11.6, 2.0, 15.5],
        target: [-14.2, 1.3, 11.5],
        follow: None,
    },
    Shot {
        name: "press_top",
        secs: 13.6,
        eye: [8.0, 3.8, 4.4],
        target: [6.5, 3.0, 2.5],
        follow: None,
    },
    Shot {
        name: "pier",
        secs: 13.9,
        eye: [-5.2, 4.8, -9.5],
        target: [-6.4, 4.2, -11.0],
        follow: None,
    },
    Shot {
        name: "tower",
        secs: 14.2,
        eye: [5.2, 4.6, 13.6],
        target: [3.0, 3.2, 10.5],
        follow: None,
    },
    Shot {
        name: "crates",
        secs: 14.5,
        eye: [3.8, 1.6, 1.8],
        target: [2.9, 0.7, -1.0],
        follow: None,
    },
];

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let out = std::path::PathBuf::from(args.get(1).map_or("screenshots/softbody", String::as_str));
    let (width, height) = args
        .get(2)
        .and_then(|s| s.split_once('x'))
        .and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)))
        .unwrap_or((1280u32, 720u32));
    let only: Option<Vec<&str>> = args.get(3).map(|s| s.split(',').collect());
    engine_core::log::init("info,wgpu=warn,naga=warn");

    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = width;
        w.height = height;
    }
    // 30 updates per demo second, two fixed steps each.
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
    app.world.resource_mut::<Atmosphere>().ground_color = [0.09, 0.18, 0.04];
    app.world.spawn((
        DirectionalLight {
            illuminance: 100_000.0,
            ..Default::default()
        },
        Transform::default().looking_at(DVec3::new(-0.45, -0.62, -0.65), Vec3::Y),
    ));
    let camera = app
        .world
        .spawn((
            Camera {
                exposure: Exposure::Manual { ev100: 14.6 },
                ..Camera::perspective(55.0)
            },
            Transform::from_xyz(0.0, 10.0, 30.0).looking_at(DVec3::ZERO, Vec3::Y),
        ))
        .id();
    app.add_systems(Startup, |mut commands: Commands| {
        // Packed earth and gravel under the yard, fraying into a grass field.
        engine_softbody::demo::spawn_review_ground(&mut commands, DVec3::ZERO);
        engine_softbody::demo::spawn_demo(&mut commands, DVec3::ZERO);
    });

    std::fs::create_dir_all(&out).expect("output directory");
    let mut elapsed = 0.0f32;
    for (i, shot) in SHOTS.iter().enumerate() {
        if only.as_ref().is_some_and(|o| !o.contains(&shot.name)) {
            continue;
        }
        while elapsed < shot.secs {
            app.update();
            elapsed += 1.0 / 30.0;
        }
        if let Some(d) = app.world.get_resource::<RenderDisabled>() {
            eprintln!("no GPU ({}); nothing to capture", d.0);
            return;
        }
        let (base, rot) = shot.follow.map_or((DVec3::ZERO, Quat::IDENTITY), |name| {
            let mut q = app.world.query::<(&Name, &Transform)>();
            q.iter(&app.world)
                .find(|(n, _)| n.as_str() == name)
                .map_or((DVec3::ZERO, Quat::IDENTITY), |(_, t)| {
                    (t.translation, t.rotation)
                })
        });
        let local = |p: [f64; 3]| base + (rot * DVec3::from_array(p).as_vec3()).as_dvec3();
        let t =
            Transform::from_translation(local(shot.eye)).looking_at(local(shot.target), Vec3::Y);
        *app.world.get_mut::<Transform>(camera).unwrap() = t;
        *app.world.get_mut::<GlobalTransform>(camera).unwrap() = GlobalTransform::from(t);
        let path = out.join(format!("{:02}_{}.png", i + 1, shot.name));
        match capture_now(&mut app.world, &path) {
            Ok(_) => println!("{} ({:.1} s)", path.display(), elapsed),
            Err(e) => eprintln!("{}: {e}", shot.name),
        }
    }
    let d = app.world.resource::<Diagnostics>();
    println!(
        "soft bodies: {:?}, particles: {:?}, step: {:?} ms",
        d.get("softbody.bodies"),
        d.get("softbody.particles"),
        d.get("softbody.step_ms"),
    );
}
