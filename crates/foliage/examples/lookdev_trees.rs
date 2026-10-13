//! Look-development shots of the tree species, headless, for judging crowns, leaves and
//! needles:
//!
//! ```sh
//! cargo run -p engine_foliage --example lookdev_trees --release -- /tmp/lookdev_trees [shot..]
//! ```
//!
//! Renders (640×360): a lineup of every tree species at full size (also with every tree forced
//! to its far level of detail), the oak's root flares, an oak crown lit from the front and
//! from behind (the sun shining through the leaves), single leaves up close, the conifers,
//! birch and hazel, and a small mixed grove from 40 m and from 150 m. Name shots after the
//! directory to render only those (`lineup`, `lineup_lod`, `roots`, `crown_front`,
//! `leaves_close`, `crown_back`, `conifers`, `birch_hazel`, `grove`, `grove_far`), `textures`
//! to save the leaf atlases and their normal maps instead, or `stats` to print triangle counts.

use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_foliage::{EcologySettings, FoliagePlugin, Genes, spawn_plant};
use engine_render::prelude::*;
use engine_render::{Exposure, Screenshots};
use engine_world::WeatherState;

const STEP: f64 = 1.0 / 30.0;

/// Point the sun so it shines along `-to_sun` (`to_sun` points from the ground to the sun).
fn set_sun(world: &mut World, to_sun: Vec3) {
    let dir = to_sun.normalize();
    world.resource_mut::<WeatherState>().sun_direction = dir;
    let mut q = world.query_filtered::<&mut Transform, With<DirectionalLight>>();
    for mut t in q.iter_mut(world) {
        let mut sun = Transform::default();
        sun.look_to(-dir, Vec3::Y);
        *t = sun;
    }
}

fn aim(world: &mut World, eye: DVec3, target: DVec3) {
    let mut q = world.query_filtered::<&mut Transform, With<Camera>>();
    for mut t in q.iter_mut(world) {
        *t = Transform::from_translation(eye).looking_at(target, Vec3::Y);
    }
}

fn shot(app: &mut App, out: &str, name: &str, eye: DVec3, target: DVec3, to_sun: Vec3) {
    aim(&mut app.world, eye, target);
    set_sun(&mut app.world, to_sun);
    // Let auto exposure settle.
    app.update_n(5);
    let path = format!("{out}/{name}.png");
    app.world
        .resource_mut::<Screenshots>()
        .request(path.clone());
    app.update();
    println!("saved {path}");
}

/// Save every tree leaf atlas (color with alpha, and normal map) as PNGs.
fn save_textures(out: &str) {
    use engine_foliage::meshgen::textures::{LeafKind, leaf_textures};
    for (name, kind) in [
        ("oak", LeafKind::Lobed),
        ("hazel", LeafKind::Broad),
        ("birch", LeafKind::Small),
        ("pine", LeafKind::LongNeedles),
        ("spruce", LeafKind::ShortNeedles),
    ] {
        let started = std::time::Instant::now();
        let t = leaf_textures(kind, 1024, 0x1eaf);
        println!(
            "{name}: painted in {:.2} s",
            started.elapsed().as_secs_f32()
        );
        for (suffix, img) in [("color", &t.color), ("normal", &t.normal)] {
            let path = format!("{out}/leaves_{name}_{suffix}.png");
            image::RgbaImage::from_raw(img.width, img.height, img.data.clone())
                .expect("rgba")
                .save(&path)
                .expect("png");
            println!("saved {path}");
        }
    }
}

/// Triangles of every tree species' near and far meshes (mean over the mesh variants).
fn print_stats() {
    use engine_foliage::meshgen::tree::woody;
    let library = engine_foliage::PlantLibrary::default();
    for name in ["oak", "birch", "hazel", "pine", "spruce"] {
        let sp = library.get(library.id(name).unwrap());
        let tris = |m: &Option<engine_render::Mesh>| m.as_ref().map_or(0, |m| m.triangle_count());
        let (mut near, mut far, mut leaves) = (0, 0, 0);
        for seed in 0..4u64 {
            let m = woody(sp, seed * 0x9e37_79b9, false);
            near += tris(&m.wood) + tris(&m.foliage) + tris(&m.base);
            far += tris(&m.wood_lod) + tris(&m.foliage_lod) + tris(&m.base);
            leaves += tris(&m.foliage);
        }
        println!(
            "{name:7} near {:6} (leaves {:6})  far {:5}",
            near / 4,
            leaves / 4,
            far / 4
        );
    }
}

fn genes(variant: u8, hue: f32) -> Option<Genes> {
    Some(Genes {
        variant,
        hue,
        ..Default::default()
    })
}

fn main() {
    engine_core::log::init("info,wgpu=warn,naga=warn");
    let mut args = std::env::args().skip(1);
    let out = args.next().unwrap_or_else(|| "/tmp/lookdev_trees".into());
    let only: Vec<String> = args.collect();
    let wanted = |name: &str| only.is_empty() || only.iter().any(|o| o == name);
    std::fs::create_dir_all(&out).expect("output directory");
    if only.iter().any(|o| o == "textures") {
        save_textures(&out);
        return;
    }
    if only.iter().any(|o| o == "stats") {
        print_stats();
        return;
    }

    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = 640;
        w.height = 360;
    }
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual { step_secs: STEP });
    app.add_plugins(engine_world::WorldPlugin)
        .add_plugins(RenderPlugin)
        .add_plugins(FoliagePlugin);
    app.world.resource_mut::<EcologySettings>().paused = true;
    app.world.resource_mut::<Atmosphere>().ground_color = [0.09, 0.16, 0.05];
    app.world.spawn((
        Transform::default(),
        MeshRenderer::new(
            Shape::Plane {
                size: [2000.0, 2000.0],
                subdivisions: 1,
            },
            StandardMaterial::hex(0x4f5f33).with_roughness(0.95),
        ),
    ));
    app.world.spawn((
        DirectionalLight {
            illuminance: 100_000.0,
            ..Default::default()
        },
        Transform::default(),
    ));
    app.world.spawn((
        Camera {
            projection: Projection::Perspective {
                fov_y_deg: 50.0,
                near: 0.1,
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

    // The lineup, left to right along x at z = 0.
    let lineup = [
        ("oak", -30.0),
        ("birch", -15.0),
        ("hazel", -4.0),
        ("pine", 7.0),
        ("spruce", 19.0),
    ];
    for (name, x) in lineup {
        spawn_plant(
            &mut app.world,
            name,
            DVec3::new(x, 0.0, 0.0),
            1.0,
            genes(0, 0.0),
        )
        .unwrap();
    }
    // A grove 300 m away: mixed species, varied genes.
    let grove = DVec3::new(0.0, 0.0, -300.0);
    let mut rng = Rng::new(11);
    let mut placed: Vec<DVec3> = Vec::new();
    let kinds = [
        "oak", "oak", "birch", "birch", "pine", "spruce", "spruce", "hazel", "hazel",
    ];
    let mut tries = 0;
    while placed.len() < 26 && tries < 2000 {
        tries += 1;
        let p = grove
            + DVec3::new(
                rng.range(-22.0, 22.0) as f64,
                0.0,
                rng.range(-16.0, 16.0) as f64,
            );
        if placed.iter().any(|q| q.distance(p) < 5.5) {
            continue;
        }
        let name = kinds[rng.range_i32(0, kinds.len() as i32) as usize];
        let g = Genes {
            variant: rng.range_i32(0, 4) as u8,
            hue: rng.range(-1.0, 1.0),
            height: rng.range(0.85, 1.12),
            crown: rng.range(0.85, 1.15),
            ..Default::default()
        };
        let size = if name == "hazel" {
            1.0
        } else {
            rng.range(0.7, 1.0)
        };
        spawn_plant(&mut app.world, name, p, size, Some(g)).unwrap();
        placed.push(p);
    }
    app.world.flush();
    app.update();

    // Late-morning sun from behind the camera's left.
    let front_sun = Vec3::new(-0.45, 0.62, 0.65);
    if wanted("lineup") {
        shot(
            &mut app,
            &out,
            "lineup",
            DVec3::new(-5.0, 4.0, 52.0),
            DVec3::new(-5.0, 8.0, 0.0),
            front_sun,
        );
    }
    if wanted("lineup_lod") {
        // The same lineup with every tree switched to its far level of detail.
        app.world
            .resource_mut::<EcologySettings>()
            .tree_lod_distance_m = 1.0;
        shot(
            &mut app,
            &out,
            "lineup_lod",
            DVec3::new(-5.0, 4.0, 52.0),
            DVec3::new(-5.0, 8.0, 0.0),
            front_sun,
        );
        app.world
            .resource_mut::<EcologySettings>()
            .tree_lod_distance_m = 70.0;
    }
    if wanted("roots") {
        // The oak's foot: root flares and the bark where the trunk meets the ground.
        shot(
            &mut app,
            &out,
            "roots",
            DVec3::new(-27.4, 1.5, 4.6),
            DVec3::new(-30.0, 0.7, 0.0),
            front_sun,
        );
    }
    if wanted("crown_front") {
        shot(
            &mut app,
            &out,
            "crown_front",
            DVec3::new(-26.0, 9.0, 14.0),
            DVec3::new(-30.0, 11.5, 0.0),
            front_sun,
        );
    }
    if wanted("leaves_close") {
        // Close enough to see single leaves on the oak's sunlit side.
        shot(
            &mut app,
            &out,
            "leaves_close",
            DVec3::new(-31.0, 7.5, 9.5),
            DVec3::new(-30.0, 8.5, 0.0),
            front_sun,
        );
    }
    if wanted("crown_back") {
        // Low sun behind the oak, shining through its crown toward the camera.
        shot(
            &mut app,
            &out,
            "crown_back",
            DVec3::new(-26.0, 6.0, 15.0),
            DVec3::new(-30.0, 12.0, 0.0),
            Vec3::new(-0.15, 0.35, -1.0),
        );
    }
    if wanted("conifers") {
        shot(
            &mut app,
            &out,
            "conifers",
            DVec3::new(13.0, 5.0, 24.0),
            DVec3::new(13.0, 8.5, 0.0),
            front_sun,
        );
    }
    if wanted("birch_hazel") {
        shot(
            &mut app,
            &out,
            "birch_hazel",
            DVec3::new(-10.0, 3.0, 17.0),
            DVec3::new(-11.0, 5.5, 0.0),
            front_sun,
        );
    }
    if wanted("grove") {
        shot(
            &mut app,
            &out,
            "grove",
            grove + DVec3::new(6.0, 3.0, 40.0 + 16.0),
            grove + DVec3::new(0.0, 7.0, 0.0),
            Vec3::new(-0.6, 0.5, 0.35),
        );
    }
    if wanted("grove_far") {
        // Far enough that every tree draws its far level of detail.
        shot(
            &mut app,
            &out,
            "grove_far",
            grove + DVec3::new(10.0, 6.0, 150.0),
            grove + DVec3::new(0.0, 6.0, 0.0),
            Vec3::new(-0.6, 0.5, 0.35),
        );
    }
}
