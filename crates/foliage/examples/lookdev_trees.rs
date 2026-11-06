//! Look-development shots of the tree species, headless, for judging crowns, leaves and
//! needles:
//!
//! ```sh
//! cargo run -p engine_foliage --example lookdev_trees --release -- /tmp/lookdev_trees [shot..]
//! ```
//!
//! Renders (640×360, or `LOOKDEV_SIZE=1280x720`; `LOOKDEV_ONLY=pine` plants only that species
//! of the lineup): a lineup of every tree species at full size (also with every tree forced
//! to its far level of detail, and to its distant one), the oak's root flares, an oak crown lit from the front and
//! from behind (the sun shining through the leaves), single leaves up close, the conifers,
//! birch and hazel, the pine's crown from below, a small mixed grove from 40 m and from
//! 150 m, and a wood of 12,000 scenery trees from a hill (with and without the distant level). Name shots after the directory to render
//! only those (`lineup`, `lineup_near`, `lineup_mid`, `lineup_lod`, `lineup_distant`, `roots`, `crown_front`,
//! `leaves_close`, `crown_back`, `conifers`, `lineup_stands` (the stands' other eight trees), `pine_crown`, `birch_hazel`, `grove`,
//! `grove_far`, `savanna`, `savanna_side`, `savanna_high`, `prairie`, `rainforest`, `rainforest_below`, `beach`, `beach_crown`, `beach_stem`, `canada`, `canada_close`, `norway`, `norway_close`, `valley`, `valley_far`, `far_woods`, `far_woods_off`), `textures`
//! to save the leaf atlases, their normal maps, three of their mip levels and the mass atlases far
//! crowns draw with instead, or `stats` to print triangle counts.

use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_foliage::{
    EcologySettings, FoliagePlugin, Genes, PlantLibrary, SceneryTree, spawn_plant,
};
use engine_render::prelude::*;
use engine_render::{Exposure, GrassField, GrassStyle, Screenshots};
use engine_world::WeatherState;

const STEP: f64 = 1.0 / 30.0;

/// The trees the stands add (engine_foliage::stands), left to right along x.
const STAND_LINEUP: [(&str, f64); 8] = [
    ("beech", -44.0),
    ("alder", -28.0),
    ("aspen", -17.0),
    ("larch", -6.0),
    ("mountain birch", 5.0),
    ("rowan", 15.0),
    ("goat willow", 25.0),
    ("juniper", 34.0),
];
const STAND_LINEUP_Z: f64 = 200.0;

/// A grass field `size` meters across in patches: thicker and fresher in hollows, thinner
/// and drier on rises (as lookdev_grass's).
fn grass(style: GrassStyle, size: f32) -> GrassField {
    let points = size as u32 + 1;
    GrassField::flat(size, size)
        .sample([points, points], |x, z| {
            let n =
                engine_core::math::noise::fbm2(x as f64 * 0.045, z as f64 * 0.045, 11, 3, 2.0, 0.5);
            let density = (0.75 + 0.35 * n).clamp(0.15, 1.0);
            let dryness = (style.dryness + 0.25 * (-n)).clamp(0.0, 1.0);
            (0.0, density, dryness)
        })
        .with_style(style)
}

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
    // Let auto exposure settle, and every plant pick its level of detail for the new view
    // (they all do every eighth frame) and finish fading into it: after a jump of hundreds of
    // meters, nine frames still showed the far level's big cards.
    app.update_n(32);
    let path = format!("{out}/{name}.png");
    app.world
        .resource_mut::<Screenshots>()
        .request(path.clone());
    app.update();
    println!("saved {path}");
}

/// Save every tree leaf atlas (color with alpha, and normal map) as PNGs.
fn save_textures(out: &str) {
    use engine_foliage::meshgen::textures::{
        LeafKind, MASS_ATLAS_SIZE, leaf_textures, mass_textures,
    };
    for (name, kind) in [
        ("oak", LeafKind::Lobed),
        ("hazel", LeafKind::Broad),
        ("birch", LeafKind::Small),
        ("pine", LeafKind::LongNeedles),
        ("spruce", LeafKind::ShortNeedles),
        ("beech", LeafKind::Oval),
        ("aspen", LeafKind::Round),
        ("rowan", LeafKind::Pinnate),
        ("larch", LeafKind::SoftNeedles),
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
        // The mass atlas far and distant crowns draw with.
        let m = mass_textures(kind, MASS_ATLAS_SIZE, 0x1eaf);
        for (suffix, img) in [("color", &m.color), ("normal", &m.normal)] {
            let path = format!("{out}/mass_{name}_{suffix}.png");
            image::RgbaImage::from_raw(img.width, img.height, img.data.clone())
                .expect("rgba")
                .save(&path)
                .expect("png");
            println!("saved {path}");
        }
        // The color's mip levels a crown 10-50 m away samples (256, 128 and 64 pixels).
        for level in 1..4 {
            let size = t.color.width >> (level + 1);
            let path = format!("{out}/leaves_{name}_color_mip{}.png", level + 1);
            image::RgbaImage::from_raw(size, size, t.color.mips[level].clone())
                .expect("rgba")
                .save(&path)
                .expect("png");
            println!("saved {path}");
        }
    }
}

/// Triangles of every tree species' near, mid, far and distant meshes (mean over the mesh variants).
fn print_stats() {
    use engine_foliage::meshgen::tree::woody;
    let library = engine_foliage::PlantLibrary::default();
    let names = ["oak", "birch", "hazel", "pine", "spruce"]
        .into_iter()
        .chain(STAND_LINEUP.map(|(n, _)| n));
    for name in names {
        let sp = library.get(library.id(name).unwrap());
        let tris = |m: &Option<engine_render::Mesh>| m.as_ref().map_or(0, |m| m.triangle_count());
        let (mut near, mut mid, mut far, mut distant, mut leaves, mut hero) = (0, 0, 0, 0, 0, 0);
        for seed in 0..4u64 {
            let m = woody(sp, seed * 0x9e37_79b9, false);
            near += tris(&m.wood) + tris(&m.foliage) + tris(&m.base);
            mid += tris(&m.wood_mid) + tris(&m.foliage_mid) + tris(&m.base);
            far += tris(&m.wood_lod) + tris(&m.foliage_lod) + tris(&m.base);
            distant += tris(&m.wood_distant) + tris(&m.foliage_distant);
            leaves += tris(&m.foliage);
            hero += tris(&m.foliage_hero);
        }
        println!(
            "{name:14} near {:6} (leaves {:6})  leaf by leaf {:6}  mid {:5}  far {:5}  distant {:4}",
            near / 4,
            leaves / 4,
            hero / 4,
            mid / 4,
            far / 4,
            distant / 4
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
        // LOOKDEV_SIZE=1280x720 renders bigger shots.
        let size = std::env::var("LOOKDEV_SIZE").ok();
        let (width, height) = size
            .as_deref()
            .and_then(|s| s.split_once('x'))
            .and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)))
            .unwrap_or((640, 360));
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
    let only_sp = std::env::var("LOOKDEV_ONLY").ok();
    for (name, x) in lineup {
        if only_sp.as_deref().is_some_and(|o| o != name) {
            continue;
        }
        spawn_plant(
            &mut app.world,
            name,
            DVec3::new(x, 0.0, 0.0),
            1.0,
            genes(0, 0.0),
        )
        .unwrap();
    }
    // The stands' other trees, in a second lineup 200 m behind the camera's side.
    for (name, x) in STAND_LINEUP {
        if only_sp.as_deref().is_some_and(|o| o != name) {
            continue;
        }
        spawn_plant(
            &mut app.world,
            name,
            DVec3::new(x, 0.0, STAND_LINEUP_Z),
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
    // A patch of dry savanna 600 m off to the side: umbrella thorns on tawny grass.
    let savanna = DVec3::new(600.0, 0.0, 0.0);
    if only_sp.as_deref().is_none_or(|o| o == "umbrella thorn") {
        app.world.spawn((
            Transform::from_translation(savanna + DVec3::Y * 0.03),
            MeshRenderer::new(
                Shape::Plane {
                    size: [400.0, 400.0],
                    subdivisions: 1,
                },
                StandardMaterial::hex(0x9c8758).with_roughness(0.95),
            ),
        ));
        // Dry-season tussock grass, tawny, round the trees.
        app.world.spawn((
            Transform::from_translation(savanna),
            grass(
                GrassStyle {
                    draw_distance: 150.0,
                    shadow_distance: 22.0,
                    ..GrassStyle::savanna()
                },
                220.0,
            ),
        ));
        for (k, (dx, dz)) in [(0.0, 0.0), (-26.0, -34.0), (31.0, -58.0), (-70.0, -95.0)]
            .into_iter()
            .enumerate()
        {
            let g = Genes {
                variant: k as u8 % 4,
                height: [1.0, 0.9, 1.1, 0.95][k],
                ..Default::default()
            };
            spawn_plant(
                &mut app.world,
                "umbrella thorn",
                savanna + DVec3::new(dx, 0.0, dz),
                1.0,
                Some(g),
            )
            .unwrap();
        }
    }
    // An oak savanna 600 m behind: bur oaks spread far apart on prairie grass.
    let prairie = DVec3::new(0.0, 0.0, 600.0);
    if only_sp.as_deref().is_none_or(|o| o == "bur oak") {
        app.world.spawn((
            Transform::from_translation(prairie + DVec3::Y * 0.03),
            MeshRenderer::new(
                Shape::Plane {
                    size: [500.0, 500.0],
                    subdivisions: 1,
                },
                StandardMaterial::hex(0x7d7f45).with_roughness(0.95),
            ),
        ));
        // Tallgrass prairie in late summer: tall, going gold.
        let mut prairie_grass = GrassStyle::savanna();
        prairie_grass.height = 1.0;
        prairie_grass.dryness = 0.35;
        prairie_grass.lush_color = [0.12, 0.15, 0.05];
        prairie_grass.draw_distance = 150.0;
        prairie_grass.shadow_distance = 22.0;
        app.world.spawn((
            Transform::from_translation(prairie),
            grass(prairie_grass, 220.0),
        ));
        for (k, (dx, dz)) in [(0.0, 0.0), (-48.0, -40.0), (52.0, -75.0), (-15.0, -130.0)]
            .into_iter()
            .enumerate()
        {
            let g = Genes {
                variant: k as u8 % 4,
                height: [1.0, 0.92, 1.05, 0.95][k],
                ..Default::default()
            };
            spawn_plant(
                &mut app.world,
                "bur oak",
                prairie + DVec3::new(dx, 0.0, dz),
                1.0,
                Some(g),
            )
            .unwrap();
        }
    }
    // A patch of rainforest 600 m to the other side: meranti emergents over a closed canopy.
    let rainforest = DVec3::new(-600.0, 0.0, 0.0);
    if only_sp
        .as_deref()
        .is_none_or(|o| o == "meranti" || o == "tropical canopy")
    {
        app.world.spawn((
            Transform::from_translation(rainforest + DVec3::Y * 0.03),
            MeshRenderer::new(
                Shape::Plane {
                    size: [300.0, 300.0],
                    subdivisions: 1,
                },
                StandardMaterial::hex(0x2c3a1c).with_roughness(0.95),
            ),
        ));
        let mut rng = Rng::new(23);
        for i in 0..14 {
            for j in 0..14 {
                let p = rainforest
                    + DVec3::new(
                        (i as f64 - 6.5) * 9.0 + rng.range(-2.5, 2.5) as f64,
                        0.0,
                        (j as f64 - 6.5) * 9.0 + rng.range(-2.5, 2.5) as f64,
                    );
                let g = Genes {
                    variant: rng.range_i32(0, 4) as u8,
                    hue: rng.range(-1.0, 1.0),
                    height: rng.range(0.8, 1.1),
                    crown: rng.range(0.85, 1.2),
                    ..Default::default()
                };
                let size = rng.range(0.8, 1.0);
                spawn_plant(&mut app.world, "tropical canopy", p, size, Some(g)).unwrap();
            }
        }
        for (k, (dx, dz)) in [
            (-20.0, 10.0),
            (25.0, -15.0),
            (-35.0, -40.0),
            (10.0, -50.0),
            (40.0, 30.0),
            (-50.0, 45.0),
        ]
        .into_iter()
        .enumerate()
        {
            let g = Genes {
                variant: k as u8 % 4,
                height: [1.15, 1.3, 1.1, 1.25, 1.15, 1.35][k],
                ..Default::default()
            };
            spawn_plant(
                &mut app.world,
                "meranti",
                rainforest + DVec3::new(dx, 0.0, dz),
                1.0,
                Some(g),
            )
            .unwrap();
        }
    }
    // A tropical shore 600 m ahead: coconut palms leaning out over the sand toward the sea.
    let beach = DVec3::new(0.0, 0.0, -600.0);
    let mut palms = Vec::new();
    if only_sp.as_deref().is_none_or(|o| o == "coconut palm") {
        app.world.spawn((
            Transform::from_translation(beach + DVec3::new(-60.0, 0.03, 0.0)),
            MeshRenderer::new(
                Shape::Plane {
                    size: [160.0, 400.0],
                    subdivisions: 1,
                },
                StandardMaterial::hex(0xcdb98e).with_roughness(0.95),
            ),
        ));
        app.world.spawn((
            Transform::from_translation(beach + DVec3::new(120.0, 0.0, 0.0)),
            MeshRenderer::new(
                Shape::Plane {
                    size: [200.0, 400.0],
                    subdivisions: 1,
                },
                StandardMaterial::hex(0x2b7f86).with_roughness(0.08),
            ),
        ));
        let mut rng = Rng::new(41);
        for k in 0..9 {
            let g = Genes {
                variant: k as u8 % 4,
                height: rng.range(0.8, 1.15),
                ..Default::default()
            };
            let p = beach
                + DVec3::new(
                    rng.range(-14.0, 4.0) as f64,
                    0.0,
                    (k as f64 - 4.0) * 11.0 + rng.range(-3.0, 3.0) as f64,
                );
            spawn_plant(&mut app.world, "coconut palm", p, 1.0, Some(g)).unwrap();
            palms.push(p);
        }
    }
    // A Canadian Shield lake shore 600 m off diagonally: a wall of black spruce spires with
    // sugar maples in front, shot in October.
    let canada = DVec3::new(600.0, 0.0, 600.0);
    if only_sp
        .as_deref()
        .is_none_or(|o| o == "black spruce" || o == "sugar maple")
    {
        app.world.spawn((
            Transform::from_translation(canada + DVec3::new(0.0, 0.03, -100.0)),
            MeshRenderer::new(
                Shape::Plane {
                    size: [300.0, 200.0],
                    subdivisions: 1,
                },
                StandardMaterial::hex(0x4a4a2c).with_roughness(0.95),
            ),
        ));
        app.world.spawn((
            Transform::from_translation(canada + DVec3::new(0.0, 0.08, 60.0)),
            MeshRenderer::new(
                Shape::Plane {
                    size: [300.0, 120.0],
                    subdivisions: 1,
                },
                StandardMaterial::hex(0x1d3440).with_roughness(0.05),
            ),
        ));
        let mut rng = Rng::new(57);
        for i in 0..44 {
            for j in 0..10 {
                let p = canada
                    + DVec3::new(
                        (i as f64 - 21.5) * 3.2 + rng.range(-1.2, 1.2) as f64,
                        0.0,
                        -8.0 - j as f64 * 3.4 + rng.range(-1.2, 1.2) as f64,
                    );
                // Maples crowd the shore's warm, better-drained strip, spruce the rest.
                let maple = j < 4 && rng.chance(0.18);
                if maple && (i + j) % 2 == 1 {
                    continue;
                }
                let g = Genes {
                    variant: rng.range_i32(0, 4) as u8,
                    hue: rng.range(-1.0, 1.0),
                    height: rng.range(0.75, 1.15),
                    crown: rng.range(0.85, 1.15),
                    ..Default::default()
                };
                let name = if maple { "sugar maple" } else { "black spruce" };
                spawn_plant(&mut app.world, name, p, rng.range(0.75, 1.0), Some(g)).unwrap();
            }
        }
    }
    // A Norwegian fjord shore 600 m off the other diagonal: dark spruce and pine up the slope
    // with birches turned gold among them and along the water, a strip of autumn meadow on the
    // shore (shot with Canada's October day).
    let norway = DVec3::new(-600.0, 0.0, 600.0);
    if only_sp
        .as_deref()
        .is_none_or(|o| o == "birch" || o == "spruce" || o == "pine" || o == "rowan")
    {
        app.world.spawn((
            Transform::from_translation(norway + DVec3::new(0.0, 0.03, -100.0)),
            MeshRenderer::new(
                Shape::Plane {
                    size: [300.0, 200.0],
                    subdivisions: 1,
                },
                StandardMaterial::hex(0x4b4a30).with_roughness(0.95),
            ),
        ));
        app.world.spawn((
            Transform::from_translation(norway + DVec3::new(0.0, 0.08, 60.0)),
            MeshRenderer::new(
                Shape::Plane {
                    size: [300.0, 120.0],
                    subdivisions: 1,
                },
                StandardMaterial::hex(0x1a2e36).with_roughness(0.05),
            ),
        ));
        app.world.spawn((
            Transform::from_translation(norway + DVec3::new(0.0, 0.0, -4.0)),
            grass(
                GrassStyle {
                    dryness: 0.55,
                    draw_distance: 150.0,
                    shadow_distance: 22.0,
                    ..GrassStyle::meadow()
                },
                120.0,
            ),
        ));
        let mut rng = Rng::new(71);
        for i in 0..40 {
            for j in 0..10 {
                let p = norway
                    + DVec3::new(
                        (i as f64 - 19.5) * 3.6 + rng.range(-1.4, 1.4) as f64,
                        0.0,
                        -10.0 - j as f64 * 3.8 + rng.range(-1.4, 1.4) as f64,
                    );
                // Birches crowd the shore, conifers the slope behind.
                let r = rng.range(0.0, 1.0);
                let birch_share = if j < 3 { 0.6 } else { 0.25 };
                let name = if r < birch_share {
                    "birch"
                } else if r < birch_share + 0.04 {
                    "rowan"
                } else if r < birch_share + 0.16 {
                    "pine"
                } else {
                    "spruce"
                };
                let g = Genes {
                    variant: rng.range_i32(0, 4) as u8,
                    hue: rng.range(-1.0, 1.0),
                    height: rng.range(0.8, 1.15),
                    crown: rng.range(0.85, 1.15),
                    ..Default::default()
                };
                spawn_plant(&mut app.world, name, p, rng.range(0.75, 1.0), Some(g)).unwrap();
            }
        }
    }
    app.world.flush();
    app.update();
    // Wait for every species' meshes (built off the frame), so the first shot isn't empty.
    for _ in 0..2400 {
        app.update();
        if app
            .world
            .resource::<engine_foliage::FoliageLoading>()
            .is_ready()
        {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }

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
    if wanted("lineup_stands") {
        shot(
            &mut app,
            &out,
            "lineup_stands",
            DVec3::new(-4.0, 4.0, STAND_LINEUP_Z + 52.0),
            DVec3::new(-4.0, 7.0, STAND_LINEUP_Z),
            front_sun,
        );
    }
    for (name, mid) in [("lineup_near", 1000.0), ("lineup_mid", 1.0)] {
        if !only.iter().any(|o| o == name) {
            continue;
        }
        // The same lineup with every tree on its near level (as before the middle level), or
        // on its middle level.
        app.world
            .resource_mut::<EcologySettings>()
            .tree_mid_distance_m = mid;
        shot(
            &mut app,
            &out,
            name,
            DVec3::new(-5.0, 4.0, 52.0),
            DVec3::new(-5.0, 8.0, 0.0),
            front_sun,
        );
        app.world
            .resource_mut::<EcologySettings>()
            .tree_mid_distance_m = 20.0;
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
    if wanted("lineup_distant") {
        // The same lineup with every tree switched to its distant level of detail.
        app.world
            .resource_mut::<EcologySettings>()
            .tree_distant_distance_m = 1.0;
        shot(
            &mut app,
            &out,
            "lineup_distant",
            DVec3::new(-5.0, 4.0, 52.0),
            DVec3::new(-5.0, 8.0, 0.0),
            front_sun,
        );
        app.world
            .resource_mut::<EcologySettings>()
            .tree_distant_distance_m = EcologySettings::default().tree_distant_distance_m;
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
    if wanted("pine_crown") {
        // Up into the pine's crown: its limbs and the pads of needles on them.
        shot(
            &mut app,
            &out,
            "pine_crown",
            DVec3::new(13.0, 11.0, 12.0),
            DVec3::new(7.0, 16.5, 0.0),
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
    if wanted("savanna") {
        // The acacia against a low sun, as savanna photos are taken: the flat top and forked
        // trunk read as a silhouette.
        shot(
            &mut app,
            &out,
            "savanna",
            savanna + DVec3::new(4.0, 2.2, 26.0),
            savanna + DVec3::new(-6.0, 6.0, -20.0),
            Vec3::new(0.3, 0.16, -0.94),
        );
    }
    if wanted("savanna_side") {
        // The same trees side-lit from a little higher.
        shot(
            &mut app,
            &out,
            "savanna_side",
            savanna + DVec3::new(14.0, 3.0, 24.0),
            savanna + DVec3::new(-4.0, 5.0, -10.0),
            Vec3::new(-0.7, 0.45, 0.3),
        );
    }
    if wanted("savanna_high") {
        // From crown height, side-lit: the flat top's underside and the leaders under it.
        shot(
            &mut app,
            &out,
            "savanna_high",
            savanna + DVec3::new(0.0, 6.0, 30.0),
            savanna + DVec3::new(0.0, 4.5, 0.0),
            Vec3::new(-0.7, 0.45, 0.3),
        );
    }
    if wanted("prairie") {
        // Bur oaks in the evening sun from behind the camera's right.
        shot(
            &mut app,
            &out,
            "prairie",
            prairie + DVec3::new(8.0, 2.0, 32.0),
            prairie + DVec3::new(-5.0, 9.0, -20.0),
            Vec3::new(0.55, 0.22, 0.8),
        );
    }
    if wanted("rainforest") {
        // Over the canopy from a ridge, the emergents standing out of it.
        shot(
            &mut app,
            &out,
            "rainforest",
            rainforest + DVec3::new(40.0, 95.0, 130.0),
            rainforest + DVec3::new(0.0, 20.0, -10.0),
            Vec3::new(-0.5, 0.55, 0.4),
        );
    }
    if wanted("rainforest_below") {
        // A meranti's straight pale trunk from the forest floor.
        shot(
            &mut app,
            &out,
            "rainforest_below",
            rainforest + DVec3::new(-12.0, 1.7, 26.0),
            rainforest + DVec3::new(-20.0, 18.0, 10.0),
            Vec3::new(-0.5, 0.75, 0.4),
        );
    }
    if wanted("beach") {
        // Along the shore with the late sun behind the palms: fronds against the sky.
        shot(
            &mut app,
            &out,
            "beach",
            beach + DVec3::new(-30.0, 1.7, 55.0),
            beach + DVec3::new(-2.0, 12.0, 0.0),
            Vec3::new(0.6, 0.35, -0.7),
        );
    }
    if wanted("beach_crown") {
        // Up into a crown from under it, sun from the side.
        shot(
            &mut app,
            &out,
            "beach_crown",
            beach + DVec3::new(-20.0, 1.6, 16.0),
            beach + DVec3::new(-6.0, 17.0, 0.0),
            Vec3::new(-0.5, 0.6, 0.6),
        );
    }
    if wanted("beach_stem") {
        // A stem up close: the rings of shed fronds, the swollen foot in the sand.
        shot(
            &mut app,
            &out,
            "beach_stem",
            palms.first().copied().unwrap_or(beach) + DVec3::new(3.6, 1.3, 2.6),
            palms.first().copied().unwrap_or(beach) + DVec3::new(0.0, 1.6, 0.0),
            Vec3::new(-0.5, 0.6, 0.6),
        );
    }
    if wanted("canada") || wanted("canada_close") || wanted("norway") || wanted("norway_close") {
        // Early October: the maples have turned and few leaves are down yet.
        app.world
            .resource_mut::<engine_foliage::FoliageSeason>()
            .day_of_year = 279.0;
        app.update_n(60);
    }
    if wanted("canada") {
        // Across the lake from the far shore, morning sun from behind the camera's left.
        shot(
            &mut app,
            &out,
            "canada",
            canada + DVec3::new(-15.0, 2.0, 95.0),
            canada + DVec3::new(5.0, 9.0, -10.0),
            Vec3::new(-0.45, 0.35, 0.8),
        );
    }
    if wanted("canada_close") {
        // From the shore itself, a few meters off the first trees.
        shot(
            &mut app,
            &out,
            "canada_close",
            canada + DVec3::new(10.0, 1.7, 18.0),
            canada + DVec3::new(-2.0, 8.0, -12.0),
            Vec3::new(-0.45, 0.35, 0.8),
        );
    }
    if wanted("norway") {
        // Across the fjord, low morning sun from the side.
        shot(
            &mut app,
            &out,
            "norway",
            norway + DVec3::new(-15.0, 2.0, 95.0),
            norway + DVec3::new(5.0, 9.0, -10.0),
            Vec3::new(0.7, 0.3, 0.5),
        );
    }
    if wanted("norway_close") {
        // From the shore meadow, among the first birches.
        shot(
            &mut app,
            &out,
            "norway_close",
            norway + DVec3::new(8.0, 1.7, 16.0),
            norway + DVec3::new(-4.0, 7.0, -14.0),
            Vec3::new(0.7, 0.3, 0.5),
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
    if wanted("far_woods") || wanted("far_woods_off") {
        // Patchy woods painted by a stand-in terrain out to 6 km, seen from 80 m up: only the
        // far woods draw them (`far_woods_off`: none, the land the terrain paints today).
        app.world.spawn((
            Transform::from_xyz(0.0, -0.05, -3000.0),
            MeshRenderer::new(
                Shape::Plane {
                    size: [14_000.0, 14_000.0],
                    subdivisions: 1,
                },
                StandardMaterial::hex(0x4f5f33).with_roughness(0.95),
            ),
        ));
        app.world
            .insert_resource(engine_world::ForestCover(std::sync::Arc::new(PatchyWoods)));
        let eye = DVec3::new(0.0, 80.0, 400.0);
        let target = DVec3::new(0.0, 0.0, -2000.0);
        let sun = Vec3::new(-0.6, 0.5, 0.35);
        // Build every cell at once, in the frame.
        {
            let mut far = app.world.resource_mut::<engine_foliage::FarWoods>();
            far.background_builds = 0;
            far.cells_per_frame = u32::MAX;
        }
        if wanted("far_woods") {
            app.update_n(6);
            shot(&mut app, &out, "far_woods", eye, target, sun);
        }
        if wanted("far_woods_off") {
            app.world.resource_mut::<engine_foliage::FarWoods>().enabled = false;
            app.update_n(4);
            shot(&mut app, &out, "far_woods_off", eye, target, sun);
        }
    }
    if wanted("valley") || wanted("valley_far") {
        // A wood of scenery trees (no simulation) seen from a hill: the nearest draw in full,
        // then far, then distant out to a kilometer (`valley_far`: no distant level).
        spawn_wood(&mut app.world);
        let eye = DVec3::new(0.0, 35.0, 60.0);
        let target = DVec3::new(0.0, 0.0, -400.0);
        let sun = Vec3::new(-0.6, 0.5, 0.35);
        if wanted("valley") {
            app.update_n(10);
            shot(&mut app, &out, "valley", eye, target, sun);
        }
        if wanted("valley_far") {
            app.world
                .resource_mut::<EcologySettings>()
                .tree_distant_distance_m = 1.0e6;
            app.update_n(10);
            shot(&mut app, &out, "valley_far", eye, target, sun);
        }
    }
}

/// A stand-in terrain's woods: soft patches with glades, conifers to the east.
struct PatchyWoods;

impl engine_world::ForestQuery for PatchyWoods {
    fn sample(&self, p: DVec3) -> Option<engine_world::ForestSample> {
        let (x, z) = (p.x as f32, p.z as f32);
        let patch = (x * 0.0021).sin() * (z * 0.0017 + 1.3).cos()
            + 0.6 * (x * 0.0007 - z * 0.0011).sin()
            + 0.25 * (x * 0.009 + z * 0.007).sin();
        let density = ((patch + 0.2) * 1.6).clamp(0.0, 1.0);
        (density > 0.02).then(|| engine_world::ForestSample {
            density,
            conifer: (0.5 + x * 0.0003).clamp(0.1, 0.9),
        })
    }
}

/// 12,000 scenery trees in a wood from 100 m to a kilometer in front of the valley shot's hill, mostly conifers, in patches with glades between them.
fn spawn_wood(world: &mut World) {
    let library = world.resource::<PlantLibrary>().clone();
    let mut rng = Rng::new(0x0a11e7);
    let mut count = 0;
    while count < 12_000 {
        let x = rng.range(-500.0, 500.0);
        let z = rng.range(-1000.0, -40.0);
        // Keep clear of the lineup and the grove.
        if (x.abs() < 45.0 && z > -25.0) || (x.abs() < 40.0 && (z + 300.0).abs() < 30.0) {
            continue;
        }
        // Glades: a few soft holes in the wood.
        let patch = (x * 0.011).sin() * (z * 0.013 + 1.3).cos() + (x * 0.004 - z * 0.006).sin();
        if patch < -0.6 && rng.chance(0.9) {
            continue;
        }
        let pick = rng.range(0.0, 1.0);
        let name = if pick < 0.42 {
            "spruce"
        } else if pick < 0.74 {
            "pine"
        } else if pick < 0.9 {
            "birch"
        } else {
            "oak"
        };
        let mut tree = SceneryTree::new(&library, name, rng.next_u64()).unwrap();
        tree.size = rng.range(0.65, 1.0);
        let yaw = Quat::from_rotation_y(rng.range(0.0, std::f32::consts::TAU));
        world.spawn((
            Transform::from_translation(DVec3::new(x as f64, 0.0, z as f64)).with_rotation(yaw),
            tree,
        ));
        count += 1;
    }
}
