//! Renders three flags from 90 m through a long lens, simulated (the camera's focus moved
//! next to them) and swaying (left far, see `engine_softbody::sway`), a few frames each, to
//! compare the two. Software Vulkan (lavapipe) is enough.
//!
//! ```sh
//! cargo run -p engine_softbody --example far_flags --release -- [out_dir]
//! ```

use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, capture_now};
use engine_softbody::{Cloth, SoftBodySim, SoftMaterial};
use engine_world::{FocusPoints, WeatherState};

/// Where the camera's focus is: next to the flags (simulated) or at the camera (far).
#[derive(Resource)]
struct Focus(DVec3);

const EYE: DVec3 = DVec3::new(0.0, 5.0, 90.0);

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let out = std::path::PathBuf::from(args.get(1).map_or("screenshots/far_flags", String::as_str));
    engine_core::log::init("info,wgpu=warn,naga=warn");
    for (mode, focus) in [("simulated", DVec3::new(0.0, 5.0, 8.0)), ("swaying", EYE)] {
        let mut app = App::new();
        app.world.resource_mut::<EngineConfig>().headless = true;
        {
            let mut w = app.world.resource_mut::<Window>();
            w.width = 960;
            w.height = 540;
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
        app.world.resource_mut::<WeatherState>().wind = Vec3::new(6.0, 0.0, 0.0);
        app.world.resource_mut::<Atmosphere>().ground_color = [0.11, 0.12, 0.07];
        app.insert_resource(Focus(focus));
        app.add_systems(
            PreUpdate,
            |f: Res<Focus>, mut points: ResMut<FocusPoints>| {
                points.points.clear();
                points.points.push(f.0);
            },
        );
        app.world.spawn((
            DirectionalLight {
                illuminance: 100_000.0,
                ..Default::default()
            },
            Transform::default().looking_at(DVec3::new(0.3, -0.7, -0.6), Vec3::Y),
        ));
        let t = Transform::from_translation(EYE).looking_at(DVec3::new(0.0, 5.0, 0.0), Vec3::Y);
        app.world.spawn((
            Camera {
                exposure: Exposure::Manual { ev100: 14.6 },
                ..Camera::perspective(7.0)
            },
            t,
        ));
        let colors = [0xc0392b, 0xf1c40f, 0x2980b9];
        app.add_systems(Startup, move |mut commands: Commands| {
            engine_softbody::demo::spawn_review_ground(&mut commands, DVec3::ZERO);
            for (i, c) in colors.iter().enumerate() {
                commands.spawn((
                    Transform::from_xyz(-4.5 + i as f64 * 4.0, 6.0 + i as f64 * 0.4, 0.0),
                    Cloth::flag([3.0, 2.0], SoftMaterial::hex(*c).with_sheen(0.25)),
                ));
            }
        });
        // Long enough to learn the sway (or settle), then four frames 0.2 s apart.
        app.update_n(240);
        if let Some(d) = app.world.get_resource::<RenderDisabled>() {
            eprintln!("no GPU ({}); nothing to capture", d.0);
            return;
        }
        std::fs::create_dir_all(&out).expect("output directory");
        for k in 0..4 {
            app.update_n(6);
            let mut q = app.world.query::<&SoftBodySim>();
            let swaying = q.iter(&app.world).filter(|b| b.swaying()).count();
            let path = out.join(format!("{mode}_{k}.png"));
            match capture_now(&mut app.world, &path) {
                Ok(_) => println!("{} ({swaying} swaying)", path.display()),
                Err(e) => eprintln!("{mode}: {e}"),
            }
        }
    }
}
