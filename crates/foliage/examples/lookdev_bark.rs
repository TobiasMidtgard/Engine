//! Bark and wood look-development: close-ups of oak, birch, pine, spruce and hazel trunks, a
//! fresh stump and a felled log, rendered headless into PNGs.
//!
//! ```sh
//! cargo run -p engine_foliage --example lookdev_bark --release -- /tmp/lookdev_bark [width height] [shots...]
//! cargo run -p engine_foliage --example lookdev_bark --release -- --dump /tmp/lookdev_bark/textures
//! ```
//!
//! `shots` picks shots by name (`oak`, `birch`, `pine`, `spruce`, `hazel`, `stump`, `log`,
//! `old_logs` for logs that have lain for years, `log_ends` for their butts and broken tops,
//! `fungi` for bracket fungi on snags, stumps and old logs, `stumps` for stumps fresh,
//! silvered and rotting, `row`, `close` for arm's-length oak, birch and pine); `--dump` saves
//! the painted bark textures instead.
//!
//! The sun rakes across the trunks from the side, which is the light that shows (or betrays)
//! bark relief. Every shot is a close-up a player could walk up to.

use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_foliage::meshgen::bark::{BarkKind, bark_textures};
use engine_foliage::{
    FelledLog, FoliagePlugin, Genes, LogPose, Plant, PlantLibrary, PlantStage, fell_tree,
    spawn_plant,
};
use engine_render::prelude::*;
use engine_render::{Exposure, RenderDisabled, Screenshots};
use engine_world::WeatherState;

const STEP: f64 = 1.0 / 15.0;

/// Direction toward the sun: low in the east-north-east, so it grazes the trunks' camera side.
fn sun_toward() -> Vec3 {
    Vec3::new(0.82, 0.42, 0.40).normalize()
}

fn aim(world: &mut World, eye: DVec3, target: DVec3) {
    let mut q = world.query_filtered::<&mut Transform, With<Camera>>();
    for mut t in q.iter_mut(world) {
        *t = Transform::from_translation(eye).looking_at(target, Vec3::Y);
    }
}

fn shot(app: &mut App, out: &str, name: &str, eye: DVec3, target: DVec3) {
    aim(&mut app.world, eye, target);
    // Let the plants and logs catch up with the camera (details near it, such as fungi, show
    // on their next full visual pass) before drawing.
    simulate(app, 16);
    app.update_n(5);
    let path = format!("{out}/{name}.png");
    app.world
        .resource_mut::<Screenshots>()
        .request(path.clone());
    app.update();
    println!("saved {path}");
}

/// Run frames without drawing them (software rendering is the slow part here).
fn simulate(app: &mut App, frames: usize) {
    let renderer = app.world.remove_resource::<engine_render::Renderer>();
    app.world
        .insert_resource(RenderDisabled("simulating only".into()));
    app.update_n(frames);
    app.world.remove_resource::<RenderDisabled>();
    if let Some(r) = renderer {
        app.world.insert_resource(r);
    }
}

/// How long a bare log is: its mesh's height along the trunk, scaled.
fn log_length(
    app: &App,
    species: engine_foliage::SpeciesId,
    variant: u8,
    scale: f32,
) -> Option<f32> {
    let vis = app
        .world
        .resource::<engine_foliage::assets::FoliageVisuals>();
    let sv = vis.species.get(species.0 as usize)?.as_ref()?;
    let v = &sv.variants[variant as usize % sv.variants.len()];
    let meshes = app
        .world
        .resource::<engine_assets::Assets<engine_render::Mesh>>();
    Some(meshes.get(v.log?)?.aabb().max.y * scale)
}

fn genes(variant: u8) -> Genes {
    Genes {
        variant,
        lean: 0.0,
        ..Default::default()
    }
}

/// Save every bark texture set as PNGs (`<kind>_albedo.png`, `_normal.png`, `_orm.png`).
fn dump_textures(out: &str) {
    std::fs::create_dir_all(out).expect("output directory");
    for kind in BarkKind::ALL {
        let started = std::time::Instant::now();
        let t = bark_textures(kind);
        println!(
            "{kind:?}: painted in {:.2} s, {:.1} MB on the GPU, mean albedo {:?}",
            started.elapsed().as_secs_f32(),
            t.gpu_bytes() as f32 / (1 << 20) as f32,
            t.mean_albedo
        );
        for (name, img) in [
            ("albedo", &t.albedo),
            ("normal", &t.normal),
            ("orm", &t.orm),
        ] {
            let path = format!("{out}/{kind:?}_{name}.png").to_lowercase();
            image::RgbaImage::from_raw(img.width, img.height, img.data.clone())
                .expect("rgba8")
                .save(&path)
                .expect("png");
        }
    }
}

fn main() {
    engine_core::log::init("info,wgpu=warn,naga=warn");
    let mut args = std::env::args().skip(1);
    let out = args.next().unwrap_or_else(|| "/tmp/lookdev_bark".into());
    if out == "--dump" {
        dump_textures(
            &args
                .next()
                .unwrap_or_else(|| "/tmp/lookdev_bark/textures".into()),
        );
        return;
    }
    let width: u32 = args.next().and_then(|a| a.parse().ok()).unwrap_or(640);
    let height: u32 = args.next().and_then(|a| a.parse().ok()).unwrap_or(360);
    let only: Vec<String> = args.collect();
    let want = |name: &str| only.is_empty() || only.iter().any(|o| name.contains(o.as_str()));
    std::fs::create_dir_all(&out).expect("output directory");

    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = width;
        w.height = height;
    }
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual { step_secs: STEP });
    app.add_plugins(engine_world::WorldPlugin)
        .add_plugins(RenderPlugin)
        .add_plugins(FoliagePlugin);

    app.world.resource_mut::<Atmosphere>().ground_color = [0.07, 0.1, 0.04];
    {
        let sun = sun_toward();
        let mut weather = app.world.resource_mut::<WeatherState>();
        weather.sun_direction = sun;
        weather.time_of_day_h = 9.0;
    }
    app.world.spawn((
        Transform::default(),
        MeshRenderer::new(
            Shape::Plane {
                size: [600.0, 600.0],
                subdivisions: 1,
            },
            StandardMaterial::hex(0x3d3426).with_roughness(0.97),
        ),
    ));
    let mut sun = Transform::default();
    sun.look_to(-sun_toward(), Vec3::Y);
    app.world.spawn((
        DirectionalLight {
            illuminance: 90_000.0,
            ..Default::default()
        },
        sun,
    ));
    app.world.spawn((
        Camera {
            projection: Projection::Perspective {
                fov_y_deg: 50.0,
                near: 0.05,
            },
            ..Default::default()
        },
        Transform::default(),
    ));
    let mut cams = app.world.query::<&mut Camera>();
    for mut cam in cams.iter_mut(&mut app.world) {
        if let Exposure::Auto { speed, .. } = &mut cam.exposure {
            *speed = 40.0;
        }
    }

    // A row of mature trees 12 m apart, and an oak to fell behind them.
    let row = [
        ("oak", 0.0, 1u8),
        ("birch", 12.0, 0),
        ("pine", 24.0, 2),
        ("spruce", 36.0, 1),
        ("hazel", 48.0, 0),
    ];
    for (name, x, v) in row {
        spawn_plant(
            &mut app.world,
            name,
            DVec3::new(x, 0.0, 0.0),
            0.95,
            Some(genes(v)),
        )
        .expect("species");
    }
    let felled_at = DVec3::new(0.0, 0.0, -24.0);
    let to_fell = spawn_plant(&mut app.world, "oak", felled_at, 0.9, Some(genes(2))).unwrap();
    app.update();

    // Fell the second oak along +X and let it settle; age it until its twigs have rotted off.
    fell_tree(&mut app.world, to_fell, Vec3::X);
    simulate(&mut app, 70);
    let mut logs = app.world.query::<(&mut FelledLog, &Transform, &LogPose)>();
    let log = logs.iter_mut(&mut app.world).next().map(|(mut l, t, _)| {
        l.years = 2.0;
        *t
    });
    // Logs that have lain for years, side by side: bare, mossy side up, sunk into the litter.
    let old_logs = DVec3::new(30.0, 0.0, -30.0);
    for (i, name) in ["oak", "birch", "pine", "spruce"].into_iter().enumerate() {
        let species = app
            .world
            .resource::<PlantLibrary>()
            .id(name)
            .expect("species");
        app.world.spawn((
            Transform::from_translation(old_logs + DVec3::new(0.0, 0.0, i as f64 * 3.2)),
            FelledLog {
                species,
                years: 3.0 + 3.0 * i as f32,
                rot_years: 16.0,
                size: 0.75,
            },
            LogPose {
                yaw: 0.06 * i as f32,
                genes: genes(i as u8),
                charred: false,
            },
        ));
    }
    // Dead wood a few years on: snags (birch, oak, spruce) and stumps (oak, birch, pine), where
    // bracket fungi fruit.
    let deadwood = DVec3::new(0.0, 0.0, 30.0);
    let dead = [
        ("birch", 0.0, PlantStage::Dead { years: 5.0 }),
        ("oak", 5.0, PlantStage::Dead { years: 5.0 }),
        ("spruce", 10.0, PlantStage::Dead { years: 5.0 }),
        ("oak", 14.0, PlantStage::Stump { years: 6.0 }),
        ("birch", 16.0, PlantStage::Stump { years: 6.0 }),
        ("pine", 18.0, PlantStage::Stump { years: 6.0 }),
    ];
    for (i, (name, x, stage)) in dead.into_iter().enumerate() {
        let e = spawn_plant(
            &mut app.world,
            name,
            deadwood + DVec3::X * x,
            0.9,
            Some(genes(i as u8)),
        )
        .expect("species");
        if let Some(mut plant) = app.world.get_mut::<Plant>(e) {
            plant.stage = stage;
        }
    }
    // Stumps of each tree through the years: fresh, silvered and rotting.
    let stumps = DVec3::new(0.0, 0.0, 50.0);
    let stump_kinds = ["oak", "birch", "pine", "spruce"];
    let stump_ages = [("fresh", 0.2), ("grey", 2.5), ("old", 7.0)];
    for (j, (_, years)) in stump_ages.into_iter().enumerate() {
        for (i, name) in stump_kinds.into_iter().enumerate() {
            let e = spawn_plant(
                &mut app.world,
                name,
                stumps + DVec3::new(i as f64 * 2.5, 0.0, j as f64 * 6.0),
                0.9,
                Some(genes(i as u8)),
            )
            .expect("species");
            if let Some(mut plant) = app.world.get_mut::<Plant>(e) {
                plant.stage = PlantStage::Stump { years };
            }
        }
    }
    simulate(&mut app, 20);

    let trunk_eye = |x: f64, d: f64, h: f64| DVec3::new(x + d * 0.62, h, d * 0.78);
    if want("oak") {
        shot(
            &mut app,
            &out,
            "01_oak",
            trunk_eye(0.0, 2.6, 1.3),
            DVec3::new(0.0, 1.05, 0.0),
        );
    }
    if want("birch") {
        shot(
            &mut app,
            &out,
            "02_birch",
            trunk_eye(12.0, 1.9, 1.2),
            DVec3::new(12.0, 1.0, 0.0),
        );
    }
    if want("pine") {
        shot(
            &mut app,
            &out,
            "03_pine",
            trunk_eye(24.0, 2.2, 1.3),
            DVec3::new(24.0, 1.05, 0.0),
        );
    }
    if want("spruce") {
        shot(
            &mut app,
            &out,
            "04_spruce",
            trunk_eye(36.0, 2.2, 1.2),
            DVec3::new(36.0, 0.9, 0.0),
        );
    }
    if want("hazel") {
        shot(
            &mut app,
            &out,
            "05_hazel",
            trunk_eye(48.0, 2.4, 1.1),
            DVec3::new(48.0, 0.8, 0.0),
        );
    }
    if want("stump") {
        let stump = felled_at;
        shot(
            &mut app,
            &out,
            "06_stump",
            stump + DVec3::new(-0.9, 1.7, 1.5),
            stump + DVec3::new(0.0, 0.35, 0.0),
        );
    }
    if let (true, Some(t)) = (want("log"), log) {
        let axis = (t.rotation * Vec3::Y).as_dvec3();
        let cut = t.translation;
        let side = DVec3::Y.cross(axis).normalize();
        let butt = cut + axis * 0.6;
        shot(
            &mut app,
            &out,
            "07_log",
            butt - axis * 2.2 + side * 1.4 + DVec3::Y * 1.0,
            butt + axis * 0.9,
        );
    }
    if want("old_logs") {
        shot(
            &mut app,
            &out,
            "12_old_logs",
            old_logs + DVec3::new(-2.5, 1.8, -3.5),
            old_logs + DVec3::new(5.0, 0.2, 4.0),
        );
        // The same row from its sunlit side, at the distance a walker passes it.
        shot(
            &mut app,
            &out,
            "12_old_logs_lit",
            old_logs + DVec3::new(5.5, 1.5, 9.5),
            old_logs + DVec3::new(1.5, 0.3, 4.8),
        );
    }
    if want("log_ends") {
        // The oak at the head of the row, from beside its sawn butt and its broken-off top.
        let library = app.world.resource::<PlantLibrary>().clone();
        let mut logs = app.world.query::<(&FelledLog, &LogPose, &Transform)>();
        let oak = logs
            .iter(&app.world)
            .find(|(l, _, _)| library.get(l.species).name == "oak")
            .map(|(l, p, t)| (l.species, p.genes.variant, *t));
        if let Some((species, variant, t)) = oak
            && let Some(len) = log_length(&app, species, variant, t.scale.y)
        {
            let axis = (t.rotation * Vec3::Y).as_dvec3();
            let side = DVec3::Y.cross(axis).normalize();
            let butt = t.translation;
            let top = butt + axis * len as f64;
            shot(
                &mut app,
                &out,
                "13_log_butt",
                butt - axis * 2.2 + side * 1.2 + DVec3::Y * 0.9,
                butt + axis * 0.6,
            );
            shot(
                &mut app,
                &out,
                "14_log_top",
                top + axis * 0.9 - side * 0.7 + DVec3::Y * 0.6,
                top - axis * 0.25,
            );
        }
    }
    if want("fungi") {
        // Each snag from both sides (fungi favour one side of a trunk), the stumps, and each old
        // log from its flank.
        for (i, name) in ["birch", "oak", "spruce"].into_iter().enumerate() {
            let at = deadwood + DVec3::new(i as f64 * 5.0, 0.0, 0.0);
            for (side, z) in [("s", -3.2), ("n", 3.2)] {
                shot(
                    &mut app,
                    &out,
                    &format!("15_fungi_snag_{name}_{side}"),
                    at + DVec3::new(0.5, 1.8, z * 0.75),
                    at + DVec3::new(0.0, 1.7, 0.0),
                );
            }
        }
        shot(
            &mut app,
            &out,
            "16_fungi_stumps",
            deadwood + DVec3::new(16.0, 1.3, -2.4),
            deadwood + DVec3::new(16.0, 0.3, 0.0),
        );
        let library = app.world.resource::<PlantLibrary>().clone();
        let mut logs = app.world.query::<(&FelledLog, &LogPose, &Transform)>();
        let old: Vec<_> = logs
            .iter(&app.world)
            .filter(|(l, _, _)| l.years >= 3.0)
            .map(|(l, p, t)| (l.species, p.genes.variant, *t))
            .collect();
        for (species, variant, t) in old {
            let Some(len) = log_length(&app, species, variant, t.scale.y) else {
                continue;
            };
            let axis = (t.rotation * Vec3::Y).as_dvec3();
            let side = DVec3::Y.cross(axis).normalize();
            let mid = t.translation + axis * (0.45 * len) as f64;
            shot(
                &mut app,
                &out,
                &format!("17_fungi_log_{}", library.get(species).name),
                mid - side * 2.6 - axis * 1.2 + DVec3::Y * 1.3,
                mid - axis * 0.4,
            );
        }
    }
    if want("stumps") {
        for (j, (age, _)) in stump_ages.into_iter().enumerate() {
            let row = stumps + DVec3::new(3.75, 0.0, j as f64 * 6.0);
            shot(
                &mut app,
                &out,
                &format!("19_stumps_{age}"),
                row + DVec3::new(0.0, 2.2, -4.6),
                row + DVec3::new(0.0, 0.2, 0.0),
            );
        }
        for (i, name) in [(0, "oak"), (1, "birch")] {
            let at = stumps + DVec3::new(i as f64 * 2.5, 0.0, 12.0);
            shot(
                &mut app,
                &out,
                &format!("19_stump_old_{name}_top"),
                at + DVec3::new(0.4, 1.6, -1.1),
                at + DVec3::new(0.0, 0.35, 0.0),
            );
        }
    }
    if want("fungi_close") {
        // Each bracket fungus on a plain post at arm's length, to judge shapes and colors.
        use engine_foliage::meshgen::fungi::{Fungus, Host, on_trunk};
        let kinds = [
            ("tinder_hoof", Fungus::TinderHoof),
            ("birch_polypore", Fungus::BirchPolypore),
            ("red_belt", Fungus::RedBelt),
            ("sulphur_shelf", Fungus::SulphurShelf),
            ("turkey_tail", Fungus::TurkeyTail),
        ];
        let path: Vec<Vec3> = (0..7).map(|k| Vec3::Y * (k as f32 * 0.5)).collect();
        let radius = vec![0.2; path.len()];
        let mut views = Vec::new();
        for (i, (name, kind)) in kinds.into_iter().enumerate() {
            let base = DVec3::new(60.0 + i as f64 * 5.0, 0.0, 40.0);
            let Some(mesh) = on_trunk(&path, &radius, Host::Snag, kind, (0.3, 0.45), 5) else {
                continue;
            };
            let center = mesh.aabb().center();
            let mesh = app
                .world
                .resource_mut::<engine_assets::Assets<engine_render::Mesh>>()
                .add(mesh);
            app.world.spawn((
                Transform::from_translation(base + DVec3::Y * 1.5),
                MeshRenderer::new(
                    Shape::Cylinder {
                        radius: 0.2,
                        height: 3.0,
                        segments: 32,
                    },
                    StandardMaterial::hex(0x5e574f).with_roughness(0.95),
                ),
            ));
            app.world.spawn((
                Transform::from_translation(base),
                MeshRenderer::new(
                    engine_render::MeshRef::Handle(mesh),
                    StandardMaterial::color(1.0, 1.0, 1.0)
                        .with_roughness(0.82)
                        .with_subsurface(0.15),
                ),
            ));
            let out = Vec3::new(center.x, 0.0, center.z).normalize_or(Vec3::X);
            let at = base + center.as_dvec3();
            let across = Vec3::Y.cross(out).as_dvec3();
            views.push((
                name,
                at + (out * 0.75).as_dvec3() + across * 0.35 + DVec3::Y * 0.3,
                at,
            ));
        }
        for (name, eye, target) in views {
            shot(&mut app, &out, &format!("18_fungus_{name}"), eye, target);
        }
    }
    if want("row") {
        shot(
            &mut app,
            &out,
            "08_row",
            DVec3::new(20.0, 1.7, 14.0),
            DVec3::new(20.0, 3.5, 0.0),
        );
    }
    if want("close") {
        // Arm's length, where the relief has to hold up.
        for (name, x) in [
            ("09_oak_close", 0.0),
            ("10_birch_close", 12.0),
            ("11_pine_close", 24.0),
        ] {
            shot(
                &mut app,
                &out,
                name,
                trunk_eye(x, 1.15, 1.45),
                DVec3::new(x, 1.3, 0.0),
            );
        }
    }
    let mut q = app.world.query::<&Plant>();
    println!("{} plants", q.iter(&app.world).count());
}
