//! Portals (`Portal`): a stone arch in a forest clearing opens onto a desert 3 km away, and an
//! arch in the desert opens back onto the clearing. Shots: the arch from the front, from the
//! side, the camera carried through (what the first shot shows through the arch), and the way
//! back from the desert.
//!
//! ```sh
//! cargo run -p engine_render --example portals -- <out-dir> [prefix]
//! ```

use engine_core::prelude::*;
use engine_render::capture_now;
use engine_render::prelude::*;

const W: u32 = 960;
const H: u32 = 540;
/// The arch's opening, meters.
const OPENING: Vec2 = Vec2::new(2.4, 3.6);
const FOREST: DVec3 = DVec3::ZERO;
const DESERT: DVec3 = DVec3::new(3000.0, 0.0, 0.0);
/// Which way the desert arch faces: you come out looking along its +Z.
const DESERT_TURN_DEG: f32 = 150.0;

fn arch(w: &mut World, at: DVec3, rotation: Quat) {
    let stone = StandardMaterial::preset(MaterialPreset::Rock);
    let half = OPENING.x as f64 * 0.5 + 0.3;
    for (offset, size) in [
        (DVec3::new(-half, 1.8, 0.0), [0.6, 3.6, 0.7]),
        (DVec3::new(half, 1.8, 0.0), [0.6, 3.6, 0.7]),
        (DVec3::new(0.0, 3.95, 0.0), [OPENING.x + 1.4, 0.7, 0.8]),
    ] {
        w.spawn((
            Transform::from_translation(at + rotation.as_dquat() * offset).with_rotation(rotation),
            MeshRenderer::new(
                Shape::RoundedCuboid {
                    size,
                    radius: 0.06,
                    segments: 0,
                },
                stone,
            ),
        ));
    }
}

fn world(camera: (DVec3, DVec3)) -> App {
    engine_core::log::init("warn");
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = W;
        w.height = H;
    }
    app.add_plugins(RenderPlugin);
    let w = &mut app.world;
    w.spawn((
        Camera {
            exposure: Exposure::Manual { ev100: 14.0 },
            ..Camera::perspective(55.0)
        },
        Transform::from_translation(camera.0).looking_at(camera.1, Vec3::Y),
    ));
    w.spawn((
        DirectionalLight::default(),
        Transform::default().looking_at(DVec3::new(-0.55, -0.4, -0.75), Vec3::Y),
    ));

    // Two arches, each the other's exit.
    let desert_turn = Quat::from_rotation_y(DESERT_TURN_DEG.to_radians());
    let opening_middle = DVec3::Y * (OPENING.y as f64 * 0.5);
    let forest_door = w
        .spawn(Transform::from_translation(FOREST + opening_middle))
        .id();
    let desert_door = w
        .spawn(Transform::from_translation(DESERT + opening_middle).with_rotation(desert_turn))
        .id();
    w.entity_mut(forest_door)
        .insert(Portal::new(desert_door, OPENING));
    w.entity_mut(desert_door)
        .insert(Portal::new(forest_door, OPENING));
    arch(w, FOREST, Quat::IDENTITY);
    arch(w, DESERT, desert_turn);

    // The clearing: soil, a few balls, conifers ringing it.
    w.spawn((
        Transform::from_translation(FOREST),
        MeshRenderer::new(
            Shape::Plane {
                size: [500.0, 500.0],
                subdivisions: 8,
            },
            StandardMaterial::color(0.16, 0.2, 0.09)
                .with_roughness(0.95)
                .with_detail(1.0, 3.0)
                .with_detail_in_world(),
        ),
    ));
    for (x, z, color) in [
        (3.0, 6.5, (0.75, 0.2, 0.12)),
        (-4.0, -7.0, (0.15, 0.3, 0.8)),
        (2.5, -12.0, (0.85, 0.65, 0.15)),
    ] {
        w.spawn((
            Transform::from_translation(FOREST + DVec3::new(x, 0.7, z)),
            MeshRenderer::new(
                Shape::Sphere {
                    radius: 0.7,
                    segments: 48,
                },
                StandardMaterial::color(color.0, color.1, color.2).with_roughness(0.35),
            ),
        ));
    }
    let bark = StandardMaterial::color(0.18, 0.12, 0.08).with_roughness(0.9);
    let needles = StandardMaterial::color(0.05, 0.11, 0.05).with_roughness(0.85);
    for ring in 0..3 {
        let radius = 18.0 + ring as f64 * 6.0;
        let count = 18 + ring * 6;
        for i in 0..count {
            let a = (i as f64 + 0.37 * ring as f64) / count as f64 * std::f64::consts::TAU;
            let jitter = ((i * 7 + ring * 3) % 5) as f64 * 0.8;
            let at = FOREST
                + DVec3::new(
                    (radius + jitter) * a.cos(),
                    0.0,
                    (radius + jitter) * a.sin(),
                );
            let height = 16.0 + ((i * 13 + ring * 5) % 7) as f32 * 1.5;
            w.spawn((
                Transform::from_translation(at + DVec3::Y * 2.0),
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
                Transform::from_translation(at + DVec3::Y * (2.5 + height as f64 * 0.5)),
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

    // The desert: sand, red rocks of every size, out to the mesas.
    w.spawn((
        Transform::from_translation(DESERT),
        MeshRenderer::new(
            Shape::Plane {
                size: [1200.0, 1200.0],
                subdivisions: 8,
            },
            StandardMaterial::color(0.62, 0.42, 0.26)
                .with_roughness(0.9)
                .with_detail(1.0, 2.0)
                .with_detail_in_world(),
        ),
    ));
    let red_rock = StandardMaterial::color(0.55, 0.26, 0.15).with_roughness(0.85);
    for i in 0..40u32 {
        // Spread mostly ahead of the desert arch (its +Z), nearest first.
        let t = i as f64 / 40.0;
        let distance = 8.0 + 260.0 * t * t;
        let angle = (i as f64 * 2.399).sin() * 1.1;
        let ahead = desert_turn.as_dquat() * DVec3::new(angle.sin(), 0.0, angle.cos());
        let size = (1.5 + 30.0 * t * t * (0.6 + 0.4 * ((i * 5 % 7) as f64 / 7.0))) as f32;
        w.spawn((
            Transform::from_translation(DESERT + ahead * distance),
            MeshRenderer::new(
                Shape::Rock {
                    size: [size, size * (0.6 + (i % 3) as f32 * 0.4), size * 0.8],
                    seed: i,
                    detail: 0,
                },
                red_rock,
            ),
        ));
    }
    app
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let out = std::path::PathBuf::from(args.first().cloned().unwrap_or_else(|| "portals".into()));
    let prefix = args.get(1).cloned().unwrap_or_default();
    std::fs::create_dir_all(&out).expect("output directory");

    let front = (DVec3::new(0.4, 1.7, 9.0), DVec3::new(0.0, 1.9, 0.0));
    // The front shot's camera carried through the forest arch.
    let forest =
        GlobalTransform::from_translation_rotation(FOREST + DVec3::Y * 1.8, Quat::IDENTITY);
    let desert = GlobalTransform::from_translation_rotation(
        DESERT + DVec3::Y * 1.8,
        Quat::from_rotation_y(DESERT_TURN_DEG.to_radians()),
    );
    let through = Portal::carry(&forest, &desert);
    let beyond = (
        through.transform_point3(front.0),
        through.transform_point3(front.1),
    );
    // Out in the desert, looking back at its arch.
    let ret = (
        desert.transform_point(DVec3::new(-1.5, -0.1, 8.0)),
        desert.transform_point(DVec3::new(0.0, 0.1, 0.0)),
    );
    for (camera, name) in [
        (front, "front"),
        (
            (DVec3::new(5.5, 1.6, 5.0), DVec3::new(0.0, 1.9, 0.0)),
            "side",
        ),
        (beyond, "carried_through"),
        (ret, "way_back"),
    ] {
        let mut app = world(camera);
        app.update_n(12);
        let path = out.join(format!("{prefix}portal_{name}.png"));
        capture_now(&mut app.world, &path).expect("screenshot");
        let d = app.world.resource::<Diagnostics>();
        println!(
            "{} ({} extra views, {:.1} ms on the main thread for them)",
            path.display(),
            d.get("render.extra_views").unwrap_or(0.0),
            d.get("render.extra_views_ms").unwrap_or(0.0),
        );
    }
}
