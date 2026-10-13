//! A still pond and wet stone in a forest clearing: what the reflections show when tall trees
//! ring the open ground. Saves the clearing with screen-space reflections on (the default) and
//! off (the sky fallback alone, which `specular_sky_visibility` occludes).
//!
//! ```sh
//! cargo run -p engine_render --example glossy_clearing -- <out-dir> [prefix]
//! ```

use engine_core::prelude::*;
use engine_render::capture_now;
use engine_render::prelude::*;

fn clearing(reflections: bool) -> App {
    engine_core::log::init("warn");
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = 960;
        w.height = 540;
    }
    app.add_plugins(RenderPlugin);
    app.world
        .resource_mut::<RenderSettings>()
        .screen_space_reflections = reflections;
    let w = &mut app.world;
    w.spawn((
        Camera {
            exposure: Exposure::Manual { ev100: 13.5 },
            ..Camera::perspective(60.0)
        },
        Transform::from_xyz(0.0, 1.8, 13.0).looking_at(DVec3::new(0.0, 1.0, -6.0), Vec3::Y),
    ));
    w.spawn((
        DirectionalLight::default(),
        Transform::default().looking_at(DVec3::new(0.5, -0.6, 0.6), Vec3::Y),
    ));
    // Forest floor, a pond and a slab of wet stone at its edge.
    w.spawn((
        Transform::default(),
        MeshRenderer::new(
            Shape::Plane {
                size: [600.0, 600.0],
                subdivisions: 8,
            },
            StandardMaterial::color(0.16, 0.2, 0.09)
                .with_roughness(0.95)
                .with_detail(1.0, 3.0)
                .with_detail_in_world(),
        ),
    ));
    w.spawn((
        Transform::from_xyz(0.0, 0.03, -2.0),
        MeshRenderer::new(
            Shape::Plane {
                size: [14.0, 12.0],
                subdivisions: 1,
            },
            StandardMaterial::color(0.01, 0.015, 0.012).with_roughness(0.04),
        ),
    ));
    w.spawn((
        Transform::from_xyz(5.5, 0.05, 6.0),
        MeshRenderer::new(
            Shape::Plane {
                size: [5.0, 4.0],
                subdivisions: 1,
            },
            StandardMaterial::color(0.08, 0.08, 0.08).with_roughness(0.15),
        ),
    ));
    // Conifers ringing the clearing, two and three deep.
    let bark = StandardMaterial::color(0.18, 0.12, 0.08).with_roughness(0.9);
    let needles = StandardMaterial::color(0.05, 0.11, 0.05).with_roughness(0.85);
    for ring in 0..3 {
        let radius = 17.0 + ring as f64 * 6.0;
        let count = 18 + ring * 6;
        for i in 0..count {
            let a = (i as f64 + 0.37 * ring as f64) / count as f64 * std::f64::consts::TAU;
            let jitter = ((i * 7 + ring * 3) % 5) as f64 * 0.8;
            let (x, z) = (
                (radius + jitter) * a.cos(),
                (radius + jitter) * a.sin() - 2.0,
            );
            let height = 16.0 + ((i * 13 + ring * 5) % 7) as f32 * 1.5;
            w.spawn((
                Transform::from_xyz(x, 2.0, z),
                MeshRenderer::new(
                    Shape::Cylinder {
                        radius: 0.35,
                        height: 4.0,
                        segments: 0,
                    },
                    bark,
                ),
            ));
            w.spawn((
                Transform::from_xyz(x, 2.5 + height as f64 * 0.5, z),
                MeshRenderer::new(
                    Shape::Cone {
                        radius: 3.2,
                        height,
                        segments: 0,
                    },
                    needles,
                ),
            ));
        }
    }
    app
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let out = std::path::PathBuf::from(
        args.first()
            .cloned()
            .unwrap_or_else(|| "glossy_clearing".into()),
    );
    let prefix = args.get(1).cloned().unwrap_or_default();
    std::fs::create_dir_all(&out).expect("output directory");
    for (reflections, name) in [(true, "ssr"), (false, "no_ssr")] {
        let mut app = clearing(reflections);
        app.update_n(8);
        let path = out.join(format!("{prefix}glossy_clearing_{name}.png"));
        capture_now(&mut app.world, &path).expect("screenshot");
        println!("{}", path.display());
    }
}
