//! Renders close-ups of the water test area's procedural props (boulders, boats, jetty
//! hardware, buoys, logs, the culvert, the people and the density blocks) into PNGs, without
//! a window.
//!
//! ```text
//! cargo run -p engine_water --example prop_preview --release -- [out_dir] [views]
//! ```
//!
//! `views` is a comma-separated subset of the names in `VIEWS` (default: all). Set
//! `PROP_PREVIEW_NORMALS=1` to see world-space normals instead of shading, and
//! `PROP_PREVIEW_HIDE=<Surface>` (e.g. `Bark`) to leave out every part of that surface, and
//! `PROP_PREVIEW_SHADOW=<meters>` to change the shadow distance (24 m by default; shorter
//! gives sharper shadows for close-ups).

use engine_assets::Assets;
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, capture_now};
use engine_water::demo::materials::DemoMaterials;
use engine_water::demo::props::{self, Part};
use engine_world::WorldPlugin;

/// Where each prop stands on the ground plane: (x, y, z, yaw).
const ROW_BOULDERS: f32 = 0.0;
const ROW_ROWBOAT: f32 = 9.0;
const ROW_MOTORBOAT: f32 = 17.0;
const ROW_JETTY: f32 = 26.0;
const ROW_BUOYS: f32 = 33.0;
const ROW_LOGS: f32 = 37.0;
const ROW_CULVERT: f32 = 43.0;
const ROW_PEOPLE: f32 = 50.0;
const ROW_BLOCKS: f32 = 56.0;

/// (name, eye, target), world meters.
const VIEWS: &[(&str, [f32; 3], [f32; 3])] = &[
    ("boulders", [1.2, 1.6, 5.2], [1.6, 0.25, 0.0]),
    ("boulder_close", [-0.3, 0.9, 2.0], [0.0, 0.2, 0.0]),
    ("boulder_big", [2.2, 1.4, 1.9], [3.0, 0.35, -0.6]),
    ("rowboat", [12.2, 2.2, 3.4], [9.0, 0.3, 0.0]),
    ("rowboat_in", [6.6, 2.6, 1.3], [9.3, 0.2, 0.0]),
    ("rowboat_stern", [6.2, 1.0, 1.9], [7.5, 0.35, 0.0]),
    ("rowboat_bow", [11.9, 0.75, 1.5], [10.6, 0.4, 0.0]),
    ("motorboat", [21.5, 2.0, 4.6], [17.0, 0.6, 0.0]),
    ("motorboat_stern", [13.0, 2.4, 3.6], [15.3, 0.6, 0.0]),
    ("jetty", [29.4, 2.2, 2.8], [26.6, 0.9, 0.0]),
    ("jetty_close", [27.7, 1.75, 2.1], [26.9, 1.05, 0.9]),
    ("buoys_logs", [35.0, 1.5, 3.4], [35.2, 0.3, 0.0]),
    ("buoy_close", [33.9, 0.9, 1.7], [33.0, 0.5, 0.4]),
    ("log_close", [37.9, 0.85, 1.8], [37.0, 0.15, 0.2]),
    ("culvert", [46.2, 1.6, 3.4], [43.0, 0.4, 0.0]),
    ("culvert_close", [44.4, 0.9, 1.3], [43.0, 0.35, 0.0]),
    ("people", [53.2, 1.4, 3.8], [51.2, 0.8, 0.0]),
    ("swimmer", [50.4, 1.6, 2.2], [49.5, 0.3, 0.0]),
    ("swimmer_head", [50.3, 0.9, 0.9], [49.9, 0.35, 0.1]),
    ("wader", [52.9, 1.3, 1.2], [51.2, 0.9, 0.0]),
    ("face", [51.85, 1.63, -0.15], [51.22, 1.58, 0.0]),
    ("face_close", [51.55, 1.62, -0.1], [51.28, 1.6, 0.0]),
    ("blocks", [57.6, 1.2, 2.6], [56.8, 0.25, 0.0]),
];

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let out = args
        .get(1)
        .cloned()
        .unwrap_or_else(|| "screenshots/props".into());
    let only: Option<Vec<String>> = args
        .get(2)
        .map(|s| s.split(',').map(str::to_string).collect());
    std::fs::create_dir_all(&out).expect("output directory");

    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = 1280;
        w.height = 720;
    }
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 30.0,
        });
    app.add_plugins((RenderPlugin, WorldPlugin));
    if let Some(mut s) = app.world.get_resource_mut::<RenderSettings>() {
        s.shadow_distance = std::env::var("PROP_PREVIEW_SHADOW")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(24.0);
        if std::env::var_os("PROP_PREVIEW_NORMALS").is_some() {
            s.debug_view = engine_render::DebugView::Normals;
        }
    }

    let w = &mut app.world;
    w.spawn((
        Transform::default(),
        MeshRenderer::new(
            Shape::Plane {
                size: [400.0, 400.0],
                subdivisions: 1,
            },
            StandardMaterial::hex(0x7a7259).with_roughness(0.95),
        ),
    ));
    // A low late-afternoon sun from behind the camera's left shoulder.
    let to_sun = Vec3::new(0.45, 0.42, 0.78).normalize();
    w.spawn((
        DirectionalLight {
            illuminance: 90_000.0,
            ..Default::default()
        },
        Transform::default().looking_at(-to_sun.as_dvec3(), Vec3::Y),
    ));
    let camera = w
        .spawn((
            Camera {
                exposure: Exposure::Manual { ev100: 14.6 },
                ..Camera::perspective(42.0)
            },
            Transform::from_xyz(0.0, 2.0, 6.0),
        ))
        .id();
    app.add_systems(Startup, spawn_props);

    for (key, eye, target) in VIEWS {
        if only.as_ref().is_some_and(|o| !o.iter().any(|k| k == key)) {
            continue;
        }
        let eye = Vec3::from_array(*eye).as_dvec3();
        let target = Vec3::from_array(*target).as_dvec3();
        *app.world.get_mut::<Transform>(camera).expect("camera") =
            Transform::from_translation(eye).looking_at(target, Vec3::Y);
        let started = std::time::Instant::now();
        app.update_n(2);
        if let Some(d) = app.world.get_resource::<RenderDisabled>() {
            eprintln!("no GPU: {}", d.0);
            return;
        }
        let path = format!("{out}/{key}.png");
        match capture_now(&mut app.world, &path) {
            Ok(_) => eprintln!("{path} ({:.1} s)", started.elapsed().as_secs_f32()),
            Err(e) => eprintln!("{path}: {e}"),
        }
    }
}

/// Build every prop and stand it in the row.
fn spawn_props(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    mut meshes: ResMut<Assets<Mesh>>,
) {
    // The texture generator is someone else's work in progress: fall back to the plain
    // materials rather than losing the preview if it fails.
    let materials = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        DemoMaterials::build(&mut images)
    }))
    .unwrap_or_else(|_| {
        eprintln!("texture generation failed; previewing untextured");
        DemoMaterials::default()
    });
    let mut place = |name: &str, parts: Vec<Part>, at: Vec3, yaw: f32| {
        let tris: usize = parts.iter().map(|p| p.mesh.triangle_count()).sum();
        eprintln!("{name}: {} parts, {tris} triangles", parts.len());
        let parent = commands
            .spawn((
                Name::new(name.to_string()),
                Transform::from_translation(at.as_dvec3())
                    .with_rotation(Quat::from_rotation_y(yaw)),
            ))
            .id();
        let hide = std::env::var("PROP_PREVIEW_HIDE").unwrap_or_default();
        for part in parts {
            if !hide.is_empty() && format!("{:?}", part.surface) == hide {
                continue;
            }
            let mut material = materials.get(part.surface);
            material.double_sided |= part.double_sided;
            let handle = meshes.add(part.mesh);
            commands.spawn((
                ChildOf(parent),
                Transform::default(),
                MeshRenderer::new(handle, material),
            ));
        }
    };

    for (k, (seed, size)) in [(1u64, 0.8f32), (2, 1.3), (3, 2.0), (4, 1.0)]
        .into_iter()
        .enumerate()
    {
        let x = ROW_BOULDERS + [0.0, 1.2, 3.0, 0.4][k];
        let z = [0.0, -0.9, -0.6, -2.2][k];
        place(
            "boulder",
            props::boulder(seed, size),
            Vec3::new(x, size * 0.08, z),
            seed as f32 * 1.3,
        );
    }
    place(
        "rowboat",
        props::rowboat(),
        Vec3::new(ROW_ROWBOAT, 0.34, 0.0),
        0.0,
    );
    place(
        "motorboat",
        props::motorboat(),
        Vec3::new(ROW_MOTORBOAT, 0.78, 0.0),
        0.0,
    );

    // A bit of jetty: deck slots on stringers and posts, a cleat and the ladder.
    let deck = 1.1;
    for k in 0..6 {
        place(
            "deck",
            props::deck_section(k),
            Vec3::new(ROW_JETTY + k as f32 * 0.55, deck, 0.0),
            0.0,
        );
    }
    for (k, x) in [-0.1f32, 2.85].into_iter().enumerate() {
        // Piles stand just outside the board ends, bolted to the outer stringers, and rise
        // above the deck as mooring posts.
        for side in [-1.07f32, 1.07] {
            place(
                "post",
                props::post(deck + 0.25, 0.55, k as u64 * 2 + (side > 0.0) as u64),
                Vec3::new(ROW_JETTY + x, 0.0, side),
                0.0,
            );
        }
    }
    place(
        "cleat",
        props::cleat(),
        Vec3::new(ROW_JETTY + 1.6, deck + 0.035, 0.75),
        0.0,
    );
    place(
        "ladder",
        props::ladder(deck),
        Vec3::new(ROW_JETTY + 2.75 + 0.28, deck + 0.035, 0.0),
        0.0,
    );

    for (k, seed) in [0u64, 1].into_iter().enumerate() {
        place(
            "buoy",
            props::buoy(seed),
            Vec3::new(ROW_BUOYS + k as f32 * 0.9, 0.32, 0.4 - k as f32 * 0.8),
            0.0,
        );
    }
    for (k, seed) in [5u64, 6].into_iter().enumerate() {
        place(
            "log",
            props::log(1.7, 0.17, seed),
            Vec3::new(ROW_LOGS, 0.17, 0.5 - k as f32 * 0.7),
            0.3 - k as f32 * 0.5,
        );
    }
    place(
        "culvert",
        props::culvert(),
        Vec3::new(ROW_CULVERT, 0.4, 0.0),
        0.0,
    );
    place(
        "swimmer",
        props::swimmer(),
        Vec3::new(ROW_PEOPLE - 0.8, 0.3, 0.0),
        0.0,
    );
    place(
        "wader",
        props::wader(),
        Vec3::new(ROW_PEOPLE + 1.2, 0.0, 0.0),
        0.4,
    );
    for k in 0..4 {
        let mesh = props::block(0.5);
        let surface = [
            engine_water::demo::materials::Surface::Cork,
            engine_water::demo::materials::Surface::Pine,
            engine_water::demo::materials::Surface::Ice,
            engine_water::demo::materials::Surface::BlockConcrete,
        ][k];
        place(
            "block",
            vec![Part::new(mesh, surface)],
            Vec3::new(ROW_BLOCKS + k as f32 * 0.7, 0.25, 0.0),
            k as f32 * 0.4,
        );
    }
}
