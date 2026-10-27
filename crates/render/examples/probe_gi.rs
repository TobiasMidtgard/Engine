//! Probe GI (`RenderSettings::probe_gi`): a plaster cottage with a door and a window, lit by
//! the afternoon sun through the window and at night by a lamp in the corner, each without and
//! with the probes, and the cottage from outside (which the probes should leave as it was).
//!
//! ```sh
//! cargo run -p engine_render --example probe_gi -- <out-dir> [prefix]
//! ```

use engine_core::prelude::*;
use engine_render::capture_now;
use engine_render::prelude::*;

const W: u32 = 960;
const H: u32 = 540;
/// Frames before each shot: the probes and the metered exposure settle.
const FRAMES: usize = 40;

fn block(w: &mut World, min: DVec3, max: DVec3, material: StandardMaterial) {
    w.spawn((
        Transform::from_translation((min + max) * 0.5),
        MeshRenderer::new(
            Shape::Cuboid {
                size: (max - min).as_vec3().to_array(),
            },
            material,
        ),
    ));
}

/// The cottage: inside x -4..4, z -3..3, 3 m high; a door in the south wall (+Z), a window in
/// the west wall, a red rug where the afternoon sun falls, a table and a shelf.
fn cottage(w: &mut World) {
    let plaster = StandardMaterial::preset(MaterialPreset::Plaster);
    let wood = StandardMaterial::preset(MaterialPreset::WoodPlanks);
    let roof = StandardMaterial::preset(MaterialPreset::Concrete);
    let t = 0.25;
    let (x0, x1, z0, z1, top) = (-4.0, 4.0, -3.0, 3.0, 3.0);
    let d = |x, y, z| DVec3::new(x, y, z);
    // Floor and roof, the roof overhanging.
    block(w, d(x0 - t, -0.2, z0 - t), d(x1 + t, 0.0, z1 + t), wood);
    block(
        w,
        d(x0 - 0.8, top, z0 - 0.8),
        d(x1 + 0.8, top + 0.3, z1 + 0.8),
        roof,
    );
    // North and east walls.
    block(w, d(x0 - t, 0.0, z0 - t), d(x1 + t, top, z0), plaster);
    block(w, d(x1, 0.0, z0), d(x1 + t, top, z1), plaster);
    // South wall with the door (x 0.8..2.0, 2.2 m high).
    block(w, d(x0 - t, 0.0, z1), d(0.8, top, z1 + t), plaster);
    block(w, d(2.0, 0.0, z1), d(x1 + t, top, z1 + t), plaster);
    block(w, d(0.8, 2.2, z1), d(2.0, top, z1 + t), plaster);
    // West wall with the window (z -1.0..0.6, 0.9..2.1 m up).
    block(w, d(x0 - t, 0.0, z0), d(x0, top, -1.0), plaster);
    block(w, d(x0 - t, 0.0, 0.6), d(x0, top, z1), plaster);
    block(w, d(x0 - t, 0.0, -1.0), d(x0, 0.9, 0.6), plaster);
    block(w, d(x0 - t, 2.1, -1.0), d(x0, top, 0.6), plaster);

    // The rug in the sun's patch, the table, a shelf on the north wall, pots.
    block(
        w,
        d(-3.0, 0.0, -1.3),
        d(-1.0, 0.02, 0.9),
        StandardMaterial::color(0.42, 0.09, 0.06).with_roughness(0.95),
    );
    let dark_wood = StandardMaterial::color(0.22, 0.13, 0.07).with_roughness(0.7);
    block(w, d(0.4, 0.72, -1.9), d(2.0, 0.78, -1.0), dark_wood);
    for (x, z) in [(0.5, -1.8), (1.9, -1.8), (0.5, -1.1), (1.9, -1.1)] {
        block(
            w,
            d(x - 0.04, 0.0, z - 0.04),
            d(x + 0.04, 0.72, z + 0.04),
            dark_wood,
        );
    }
    block(w, d(-1.5, 1.4, z0), d(0.5, 1.45, z0 + 0.35), dark_wood);
    for (x, color) in [
        (-1.2, (0.15, 0.3, 0.6)),
        (-0.6, (0.7, 0.55, 0.2)),
        (0.1, (0.2, 0.45, 0.2)),
    ] {
        w.spawn((
            Transform::from_translation(d(x, 1.6, z0 + 0.18)),
            MeshRenderer::new(
                Shape::Sphere {
                    radius: 0.15,
                    segments: 24,
                },
                StandardMaterial::color(color.0, color.1, color.2).with_roughness(0.4),
            ),
        ));
    }

    // Grass around it, and a few trees.
    w.spawn((
        Transform::from_translation(d(0.0, -0.05, 0.0)),
        MeshRenderer::new(
            Shape::Plane {
                size: [400.0, 400.0],
                subdivisions: 4,
            },
            StandardMaterial::color(0.12, 0.2, 0.06)
                .with_roughness(0.95)
                .with_detail(1.0, 3.0)
                .with_detail_in_world(),
        ),
    ));
    let bark = StandardMaterial::color(0.18, 0.12, 0.08).with_roughness(0.9);
    let leaves = StandardMaterial::color(0.06, 0.13, 0.05).with_roughness(0.85);
    for (x, z) in [
        (-12.0, -8.0),
        (10.0, -12.0),
        (-9.0, 11.0),
        (14.0, 6.0),
        (-18.0, 2.0),
    ] {
        block(
            w,
            d(x - 0.25, 0.0, z - 0.25),
            d(x + 0.25, 3.0, z + 0.25),
            bark,
        );
        w.spawn((
            Transform::from_translation(d(x, 6.5, z)),
            MeshRenderer::new(
                Shape::Cone {
                    radius: 2.6,
                    height: 8.0,
                    segments: 0,
                },
                leaves,
            ),
        ));
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Time {
    Afternoon,
    Night,
}

fn world(time: Time, probes: bool, camera: (DVec3, DVec3)) -> App {
    engine_core::log::init("warn");
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = W;
        w.height = H;
    }
    app.add_plugins(RenderPlugin);
    {
        let mut s = app.world.resource_mut::<RenderSettings>();
        s.probe_gi = probes;
        // Every probe each frame, so the shots settle in a few frames.
        s.gi_probe_spacing = 1.0;
        s.gi_probe_grid = [24, 12, 24];
        s.gi_probes_per_frame = 24 * 12 * 24;
    }
    let w = &mut app.world;
    w.spawn((
        Camera {
            exposure: Exposure::Auto {
                min_ev100: -3.0,
                max_ev100: 16.0,
                speed: 40.0,
                compensation: 0.0,
            },
            ..Camera::perspective(70.0)
        },
        Transform::from_translation(camera.0).looking_at(camera.1, Vec3::Y),
    ));
    match time {
        Time::Afternoon => {
            // From the west, 37° up: through the window onto the rug.
            w.spawn((
                DirectionalLight::default(),
                Transform::default().looking_at(DVec3::new(0.8, -0.6, 0.12), Vec3::Y),
            ));
        }
        Time::Night => {
            w.spawn((
                DirectionalLight {
                    illuminance: 0.25,
                    color: [0.7, 0.8, 1.0],
                    ..Default::default()
                },
                Transform::default().looking_at(DVec3::new(-0.3, -0.8, 0.4), Vec3::Y),
            ));
            // A lamp on a stand in the north-west corner.
            let at = DVec3::new(-3.4, 1.3, -2.4);
            w.spawn((
                Transform::from_translation(at),
                PointLight {
                    color: [1.0, 0.72, 0.42],
                    intensity_lm: 1200.0,
                    range: 12.0,
                    radius: 0.06,
                },
            ));
            w.spawn((
                Transform::from_translation(at),
                MeshRenderer::new(
                    Shape::Sphere {
                        radius: 0.06,
                        segments: 16,
                    },
                    StandardMaterial::color(1.0, 0.8, 0.5).with_emissive([1.0, 0.72, 0.42], 4000.0),
                ),
            ));
            block(
                w,
                DVec3::new(-3.45, 0.0, -2.45),
                DVec3::new(-3.35, 1.24, -2.35),
                StandardMaterial::color(0.1, 0.08, 0.06),
            );
        }
    }
    cottage(w);
    app
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let out = std::path::PathBuf::from(args.first().cloned().unwrap_or_else(|| "probe_gi".into()));
    let prefix = args.get(1).cloned().unwrap_or_default();
    std::fs::create_dir_all(&out).expect("output directory");

    // From the door's corner, looking at the window wall and the rug.
    let inside = (DVec3::new(3.3, 1.6, 2.5), DVec3::new(-2.5, 1.0, -1.2));
    // Outside, before the door.
    let outside = (DVec3::new(6.0, 1.7, 11.0), DVec3::new(0.0, 1.5, 0.0));
    for (time, camera, name) in [
        (Time::Afternoon, inside, "afternoon"),
        (Time::Night, inside, "night"),
        (Time::Afternoon, outside, "outside"),
    ] {
        for probes in [false, true] {
            let mut app = world(time, probes, camera);
            let started = std::time::Instant::now();
            app.update_n(FRAMES);
            let path = out.join(format!(
                "{prefix}probe_gi_{name}_{}.png",
                if probes { "on" } else { "off" }
            ));
            capture_now(&mut app.world, &path).expect("screenshot");
            let d = app.world.resource::<Diagnostics>();
            println!(
                "{} ({:.1} s for {FRAMES} frames; {} probes traced a frame, {} instances, gpu {:.1} ms)",
                path.display(),
                started.elapsed().as_secs_f32(),
                d.get("render.gi.probes_updated").unwrap_or(0.0),
                d.get("render.gi.instances").unwrap_or(0.0),
                d.get("render.gpu_ms").unwrap_or(0.0),
            );
        }
    }
}
