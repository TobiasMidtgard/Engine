//! Renders the gaff-rigged clinker dinghy offscreen, standing on cribbing in the yard, from a
//! series of viewpoints (the rig trimmed amidships, then at 35°) and saves one PNG per shot.
//! Software Vulkan (lavapipe) is enough.
//!
//! ```sh
//! cargo run -p engine_softbody --example boat_preview --release -- [out_dir] [WIDTHxHEIGHT] [shot,shot]
//! ```

use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, capture_now};
use engine_softbody::demo::{self, boat_dims as dims};

/// One screenshot, in the boat's frame (bow toward −Z, origin at the waterline amidships).
struct Shot {
    name: &'static str,
    eye: [f64; 3],
    target: [f64; 3],
    /// Vertical field of view, degrees.
    fov: f32,
    /// Rig trim (rotation about +Y), degrees.
    rig_deg: f32,
    up: [f32; 3],
}

const UP: [f32; 3] = [0.0, 1.0, 0.0];

const SHOTS: &[Shot] = &[
    Shot {
        name: "bow_quarter",
        eye: [4.6, 2.6, -5.6],
        target: [0.0, 1.9, -0.1],
        fov: 55.0,
        rig_deg: 0.0,
        up: UP,
    },
    Shot {
        name: "side",
        eye: [8.4, 2.1, 0.1],
        target: [0.0, 1.95, 0.1],
        fov: 50.0,
        rig_deg: 0.0,
        up: UP,
    },
    Shot {
        name: "hull_side",
        eye: [4.4, 0.75, -0.2],
        target: [0.0, 0.05, -0.05],
        fov: 55.0,
        rig_deg: 0.0,
        up: UP,
    },
    Shot {
        name: "stern",
        eye: [2.3, 1.9, 5.6],
        target: [0.0, 1.2, 0.8],
        fov: 55.0,
        rig_deg: 0.0,
        up: UP,
    },
    Shot {
        name: "top",
        eye: [0.0, 14.0, 0.3],
        target: [0.0, 0.0, 0.3],
        fov: 22.0,
        rig_deg: 0.0,
        up: [-1.0, 0.0, 0.0],
    },
    Shot {
        name: "interior",
        eye: [0.75, 1.3, 2.05],
        target: [0.0, 0.1, -0.5],
        fov: 55.0,
        rig_deg: 0.0,
        up: UP,
    },
    Shot {
        name: "interior_bow",
        eye: [-0.55, 1.05, -0.2],
        target: [0.1, 0.2, -1.5],
        fov: 55.0,
        rig_deg: 0.0,
        up: UP,
    },
    Shot {
        name: "rig",
        eye: [1.9, 4.55, -2.35],
        target: [0.0, 4.15, -0.55],
        fov: 42.0,
        rig_deg: 0.0,
        up: UP,
    },
    Shot {
        name: "jaws",
        eye: [0.75, 3.75, -1.85],
        target: [0.0, 3.45, -1.1],
        fov: 40.0,
        rig_deg: 0.0,
        up: UP,
    },
    Shot {
        name: "transom",
        eye: [1.0, 0.6, 3.05],
        target: [0.0, 0.15, 2.05],
        fov: 50.0,
        rig_deg: 0.0,
        up: UP,
    },
    Shot {
        name: "bow",
        eye: [1.05, 0.85, -3.05],
        target: [0.0, 0.3, -2.0],
        fov: 50.0,
        rig_deg: 0.0,
        up: UP,
    },
    Shot {
        name: "oarlock",
        eye: [1.25, 0.95, -0.15],
        target: [0.7, 0.42, -0.75],
        fov: 45.0,
        rig_deg: 0.0,
        up: UP,
    },
    Shot {
        name: "bottom",
        eye: [3.0, -0.8, -1.8],
        target: [0.0, -0.3, 0.1],
        fov: 55.0,
        rig_deg: 0.0,
        up: UP,
    },
    Shot {
        name: "bow_quarter_35",
        eye: [4.6, 2.6, -5.6],
        target: [0.0, 1.9, -0.1],
        fov: 55.0,
        rig_deg: 35.0,
        up: UP,
    },
    Shot {
        name: "top_35",
        eye: [0.0, 14.0, 0.3],
        target: [0.0, 0.0, 0.3],
        fov: 22.0,
        rig_deg: 35.0,
        up: [-1.0, 0.0, 0.0],
    },
];

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let out = std::path::PathBuf::from(args.get(1).map_or("screenshots/boat", String::as_str));
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
            Transform::from_xyz(0.0, 3.0, 10.0).looking_at(DVec3::ZERO, Vec3::Y),
        ))
        .id();
    let base = DVec3::new(0.0, dims::STAND_DWL_Y as f64, 0.0);
    app.add_systems(Startup, move |mut commands: Commands| {
        demo::spawn_review_ground(&mut commands, DVec3::ZERO);
        demo::boat_stand(&mut commands).spawn(&mut commands, Transform::default());
        let boat = demo::boat_models(&mut commands);
        let root = boat
            .hull
            .spawn(&mut commands, Transform::from_translation(base));
        // The rig and rudder hang on pivots so they can be trimmed and steered.
        let rig = commands
            .spawn((
                Name::new("Rig pivot"),
                Transform::from_xyz(0.0, 0.0, dims::MAST_Z as f64),
                ChildOf(root),
            ))
            .id();
        boat.rig.attach(&mut commands, rig, Transform::default());
        let rudder = commands
            .spawn((
                Name::new("Rudder pivot"),
                Transform::from_xyz(0.0, 0.0, dims::RUDDER_Z as f64),
                ChildOf(root),
            ))
            .id();
        boat.rudder
            .attach(&mut commands, rudder, Transform::default());
    });

    std::fs::create_dir_all(&out).expect("output directory");
    app.update();
    for (i, shot) in SHOTS.iter().enumerate() {
        if only.as_ref().is_some_and(|o| !o.contains(&shot.name)) {
            continue;
        }
        if let Some(d) = app.world.get_resource::<RenderDisabled>() {
            eprintln!("no GPU ({}); nothing to capture", d.0);
            return;
        }
        let pivot = {
            let mut q = app.world.query::<(Entity, &Name)>();
            q.iter(&app.world)
                .find(|(_, n)| n.as_str() == "Rig pivot")
                .map(|(e, _)| e)
        };
        if let Some(pivot) = pivot {
            app.world.get_mut::<Transform>(pivot).unwrap().rotation =
                Quat::from_rotation_y(shot.rig_deg.to_radians());
        }
        app.update();
        app.update();
        let t = Transform::from_translation(base + DVec3::from_array(shot.eye)).looking_at(
            base + DVec3::from_array(shot.target),
            Vec3::from_array(shot.up),
        );
        *app.world.get_mut::<Transform>(camera).unwrap() = t;
        *app.world.get_mut::<GlobalTransform>(camera).unwrap() = GlobalTransform::from(t);
        app.world.get_mut::<Camera>(camera).unwrap().projection = Projection::Perspective {
            fov_y_deg: shot.fov,
            near: 0.05,
        };
        let path = out.join(format!("{:02}_{}.png", i + 1, shot.name));
        match capture_now(&mut app.world, &path) {
            Ok(_) => println!("{}", path.display()),
            Err(e) => eprintln!("{}: {e}", shot.name),
        }
    }
}
