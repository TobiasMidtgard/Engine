//! Three gathered linen curtains side by side in a breeze, stepped at 10, 6 and 4 substeps
//! a fixed step (600, 360 and 240 Hz), to see what the curtain's default costs and buys:
//! prints each one's milliseconds a step, its hem height and self-contacts, and renders
//! them. Software Vulkan (lavapipe) is enough.
//!
//! ```sh
//! cargo run -p engine_softbody --example curtain_substeps --release -- [out_dir]
//! ```

use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, capture_now};
use engine_softbody::{Cloth, Pattern, SoftBodySim, SoftMaterial};
use engine_world::{FocusPoints, WeatherState};

const SUBSTEPS: [u32; 3] = [10, 6, 4];
const EYE: DVec3 = DVec3::new(0.0, 3.2, 17.0);

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let out = std::path::PathBuf::from(
        args.get(1)
            .map_or("screenshots/curtain_substeps", String::as_str),
    );
    engine_core::log::init("info,wgpu=warn,naga=warn");
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = 1280;
        w.height = 640;
    }
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 60.0,
        });
    app.add_plugins((
        RenderPlugin,
        engine_world::WorldPlugin,
        engine_softbody::SoftBodyPlugin,
    ));
    app.world.resource_mut::<WeatherState>().wind = Vec3::new(3.0, 0.0, 4.0);
    app.world.resource_mut::<Atmosphere>().ground_color = [0.11, 0.12, 0.07];
    app.add_systems(PreUpdate, |mut points: ResMut<FocusPoints>| {
        points.points.clear();
        points.points.push(EYE);
    });
    app.world.spawn((
        DirectionalLight {
            illuminance: 100_000.0,
            ..Default::default()
        },
        Transform::default().looking_at(DVec3::new(0.3, -0.7, -0.6), Vec3::Y),
    ));
    let t = Transform::from_translation(EYE).looking_at(DVec3::new(0.0, 3.0, 0.0), Vec3::Y);
    app.world.spawn((
        Camera {
            exposure: Exposure::Manual { ev100: 14.6 },
            ..Camera::perspective(50.0)
        },
        t,
    ));
    app.add_systems(Startup, |mut commands: Commands| {
        engine_softbody::demo::spawn_review_ground(&mut commands, DVec3::ZERO);
        for (i, n) in SUBSTEPS.iter().enumerate() {
            let linen = SoftMaterial::hex(0xe8dcc4)
                .with_pattern(Pattern::HorizontalStripes(9), 0xd8c8a8, 0xe8dcc4)
                .with_roughness(0.85)
                .with_sheen(0.3)
                .with_subsurface(0.5);
            let mut c = Cloth::curtain([4.6, 5.6], linen);
            c.sim.substeps = *n;
            commands.spawn((
                Name::new(format!("{n} substeps")),
                Transform::from_xyz(-5.2 + i as f64 * 5.2, 6.0, 0.0),
                c,
            ));
        }
    });
    // Settle (the prewarm), then 8 s in the breeze.
    let mut ms = [0.0f32; 3];
    let mut pairs = [0usize; 3];
    let frames = 480;
    app.update_n(60);
    for _ in 0..frames {
        app.update();
        let mut q = app.world.query::<(&Name, &SoftBodySim)>();
        for (name, b) in q.iter(&app.world) {
            if let Some(i) = SUBSTEPS
                .iter()
                .position(|n| name.as_str() == format!("{n} substeps"))
            {
                ms[i] += b.step_ms / frames as f32;
                pairs[i] = pairs[i].max(b.sim.self_pairs());
            }
        }
    }
    let mut q = app.world.query::<(&Name, &SoftBodySim)>();
    for (name, b) in q.iter(&app.world) {
        let Some(i) = SUBSTEPS
            .iter()
            .position(|n| name.as_str() == format!("{n} substeps"))
        else {
            continue;
        };
        let hem = (0..b.particle_count())
            .map(|k| b.world_position(k).y)
            .fold(f64::MAX, f64::min);
        println!(
            "{name}: {:.2} ms a step, hem at {hem:.3} m, up to {} self-contacts",
            ms[i], pairs[i]
        );
    }
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("no GPU ({}); nothing to capture", d.0);
        return;
    }
    std::fs::create_dir_all(&out).expect("output directory");
    for k in 0..3 {
        app.update_n(20);
        let path = out.join(format!("curtains_{k}.png"));
        match capture_now(&mut app.world, &path) {
            Ok(_) => println!("{}", path.display()),
            Err(e) => eprintln!("{e}"),
        }
    }
}
