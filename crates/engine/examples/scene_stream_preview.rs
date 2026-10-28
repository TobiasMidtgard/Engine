//! Streams one big scene document by itself ([`engine::scene_stream`]) and shows it headless:
//! a 1.6 km valley road with villages, woods, watchtowers and a castle, written as a plain
//! [`SceneDoc`] with no cells in it. The partition cuts it into 128 m cells (the castle, 240 m
//! across, into a 256 m one), their far-view proxies are baked first (a run without the
//! renderer), then the camera at the west end renders:
//!
//! - `streamed.png`: what streaming keeps in (near cells full, far ones as proxies);
//! - `all_loaded.png`: every cell in, to compare;
//! - `map.png`: the cells from above, green loaded, amber shown by a proxy, the camera and its
//!   256 m load reach in white;
//! - `moved.png`, `map_moved.png`: the camera down the road by the castle;
//! - `far_cells.png`, `far_groups.png`: from the hills 2 km west, every cell's own proxy, then
//!   the far groups that merge each kilometre's proxies into one;
//!
//! and prints the entities and assets in each case.
//!
//! `cargo run -p engine --example scene_stream_preview -- <out dir> [WxH]`

use std::path::{Path, PathBuf};

use engine::assets::{AssetAppExt, AssetTypes};
use engine::hlod::{HlodGroupProxy, HlodProxy, HlodSettings, Proxies};
use engine::scene_stream::{PlannedCell, SceneLayout, ScenePartition, stream_scene};
use engine::stream::{StreamingBudget, StreamingPlugin};
use engine_core::math::{DVec3, Quat, Vec3};
use engine_core::prelude::*;
use engine_core::scene::{EntityDoc, SceneDoc};
use engine_core::time::TimeSource;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, capture_now};
use engine_world::{CellState, StreamPlacement, StreamedCells, StreamingSource};

/// A small deterministic hash in [0, 1).
fn noise(i: u32, salt: u32) -> f64 {
    let mut h = i.wrapping_mul(0x9e37_79b9) ^ salt.wrapping_mul(0x85eb_ca6b);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2c1b_3c6d);
    h ^= h >> 12;
    f64::from(h % 10_000) / 10_000.0
}

fn component<T: Serialize>(map: &mut serde_json::Map<String, serde_json::Value>, name: &str, v: T) {
    map.insert(name.into(), serde_json::to_value(v).expect("serializes"));
}

fn part(
    name: &str,
    at: DVec3,
    turn: f32,
    shape: Shape,
    material: StandardMaterial,
    children: Vec<EntityDoc>,
) -> EntityDoc {
    let mut components = serde_json::Map::new();
    component(
        &mut components,
        "Transform",
        Transform::from_translation(at).with_rotation(Quat::from_rotation_y(turn)),
    );
    component(
        &mut components,
        "MeshRenderer",
        MeshRenderer::new(shape, material),
    );
    EntityDoc {
        name: Some(name.into()),
        components,
        children,
    }
}

fn reach(mut doc: EntityDoc, radius: f32) -> EntityDoc {
    component(
        &mut doc.components,
        "StreamPlacement",
        StreamPlacement {
            radius,
            ..Default::default()
        },
    );
    doc
}

/// The valley: ground and sun always in, everything else placed by position.
fn valley() -> SceneDoc {
    let walls = StandardMaterial::hex(0xc9bda4).with_roughness(0.85);
    let roof = StandardMaterial::hex(0x8e3b26).with_roughness(0.8);
    let stone = StandardMaterial::preset(MaterialPreset::Rock);
    let bark = StandardMaterial::preset(MaterialPreset::Bark);
    let leaves = StandardMaterial::hex(0x3d5e22).with_roughness(0.9);
    let road = StandardMaterial::hex(0x8a7a5c).with_roughness(0.95);
    let mut entities = Vec::new();

    let mut ground = part(
        "ground",
        DVec3::ZERO,
        0.0,
        Shape::Plane {
            size: [4000.0, 1600.0],
            subdivisions: 1,
        },
        StandardMaterial::hex(0x5d6b3c).with_roughness(0.95),
        Vec::new(),
    );
    component(
        &mut ground.components,
        "StreamPlacement",
        StreamPlacement {
            spatial: false,
            ..Default::default()
        },
    );
    entities.push(ground);
    let mut sun = serde_json::Map::new();
    component(
        &mut sun,
        "Transform",
        Transform::default().looking_at(DVec3::new(0.6, -0.45, -0.5), Vec3::Y),
    );
    component(
        &mut sun,
        "DirectionalLight",
        DirectionalLight {
            color: [1.0, 0.9, 0.78],
            illuminance: 75_000.0,
            ..Default::default()
        },
    );
    component(
        &mut sun,
        "StreamPlacement",
        StreamPlacement {
            spatial: false,
            ..Default::default()
        },
    );
    entities.push(EntityDoc {
        name: Some("sun".into()),
        components: sun,
        children: Vec::new(),
    });

    // The road, in 40 m slabs.
    for i in 0..42 {
        let x = -40.0 + i as f64 * 40.0;
        entities.push(part(
            "road",
            DVec3::new(x, 0.03, 0.0),
            0.0,
            Shape::Cuboid {
                size: [40.0, 0.06, 7.0],
            },
            road,
            Vec::new(),
        ));
    }
    // Villages every 400 m: houses on both sides of the road, a roof each.
    let mut n = 0;
    for v in 0..4 {
        let cx = 120.0 + v as f64 * 400.0;
        for k in 0..14 {
            n += 1;
            let side = if k % 2 == 0 { 1.0 } else { -1.0 };
            let at = DVec3::new(
                cx - 60.0 + (k / 2) as f64 * 18.0 + noise(n, 1) * 4.0,
                0.0,
                side * (14.0 + noise(n, 2) * 10.0),
            );
            let turn = (noise(n, 3) as f32 - 0.5) * 0.4;
            let (w, h, d) = (
                7.0 + noise(n, 4) as f32 * 4.0,
                4.5 + noise(n, 5) as f32 * 2.5,
                6.0 + noise(n, 6) as f32 * 2.0,
            );
            let roof = part(
                "roof",
                DVec3::new(0.0, f64::from(h) / 2.0 + 1.6, 0.0),
                std::f32::consts::FRAC_PI_4,
                Shape::Cone {
                    radius: w * 0.75,
                    height: 3.2,
                    segments: 4,
                },
                roof,
                Vec::new(),
            );
            entities.push(part(
                &format!("house {n}"),
                at + DVec3::Y * f64::from(h) / 2.0,
                turn,
                Shape::Cuboid { size: [w, h, d] },
                walls,
                vec![roof],
            ));
        }
        // A watchtower at each village's east end.
        entities.push(reach(
            part(
                "watchtower",
                DVec3::new(cx + 80.0, 11.0, -28.0),
                0.0,
                Shape::Cylinder {
                    radius: 4.0,
                    height: 22.0,
                    segments: 20,
                },
                stone,
                Vec::new(),
            ),
            12.0,
        ));
    }
    // Woods on both sides, thinning toward the road.
    for i in 0..700u32 {
        let x = -60.0 + noise(i, 7) * 1700.0;
        let side = if noise(i, 8) < 0.5 { 1.0 } else { -1.0 };
        let z = side * (35.0 + noise(i, 9).powf(0.7) * 260.0);
        let s = 0.8 + noise(i, 10) as f32 * 0.6;
        let crown = part(
            "crown",
            DVec3::new(0.0, f64::from(4.5 * s), 0.0),
            0.0,
            Shape::Sphere {
                radius: 3.0 * s,
                segments: 10,
            },
            leaves,
            Vec::new(),
        );
        entities.push(part(
            "tree",
            DVec3::new(x, f64::from(2.5 * s), z),
            0.0,
            Shape::Cylinder {
                radius: 0.35 * s,
                height: 5.0 * s,
                segments: 6,
            },
            bark,
            vec![crown],
        ));
    }
    // A castle on the hill north of the far end: 240 m across, so it loads as one piece.
    let mut castle_parts = Vec::new();
    for i in 0..12 {
        let a = i as f64 / 12.0 * std::f64::consts::TAU;
        let p = DVec3::new(a.cos() * 100.0, 9.0, a.sin() * 100.0);
        castle_parts.push(part(
            "wall",
            p,
            (-a + std::f64::consts::FRAC_PI_2) as f32,
            Shape::Cuboid {
                size: [54.0, 18.0, 5.0],
            },
            stone,
            Vec::new(),
        ));
        castle_parts.push(part(
            "turret",
            DVec3::new(a.cos() * 104.0, 13.0, a.sin() * 104.0),
            0.0,
            Shape::Cylinder {
                radius: 6.0,
                height: 26.0,
                segments: 16,
            },
            stone,
            Vec::new(),
        ));
    }
    castle_parts.push(part(
        "keep",
        DVec3::new(0.0, 20.0, 0.0),
        0.0,
        Shape::Cuboid {
            size: [36.0, 40.0, 36.0],
        },
        stone,
        Vec::new(),
    ));
    let mut castle = part(
        "castle",
        DVec3::new(1450.0, 0.0, -420.0),
        0.0,
        Shape::Cuboid {
            size: [1.0, 0.1, 1.0],
        },
        stone,
        castle_parts,
    );
    castle = reach(castle, 120.0);
    entities.push(castle);
    SceneDoc { entities }
}

/// Bakes every cell's far-view proxy into `cache`, without the renderer: each cell loads
/// alone, settles, is captured and goes.
fn bake(doc: &SceneDoc, cache: &Path) {
    let mut app = App::new();
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 30.0,
        });
    app.add_plugins((engine_world::WorldPlugin, StreamingPlugin))
        .init_asset::<Mesh>()
        .init_asset::<StandardMaterial>()
        .init_asset::<Image>()
        .register_component::<MeshRenderer>("A mesh to draw.")
        .register_component::<DirectionalLight>("The sun.");
    {
        let mut s = app.world.resource_mut::<HlodSettings>();
        s.settle_frames = 3;
        s.cache_dir = cache.to_path_buf();
        s.bake = true;
    }
    // A source far away: nothing loads unless the bake pins it.
    app.world.spawn((
        Transform::from_translation(DVec3::new(1e6, 0.0, 0.0)),
        GlobalTransform::default(),
        StreamingSource::default(),
    ));
    stream_scene(&mut app.world, "valley", doc, &ScenePartition::default()).expect("scene");
    for _ in 0..20_000 {
        app.update();
        if app.world.resource::<Proxies>().baked {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    let tris: usize = app.world.resource::<Proxies>().triangles().iter().sum();
    println!(
        "baked: {} ({tris} proxy triangles)",
        app.world.resource::<Proxies>().baked
    );
}

/// Entities alive and every asset type's count.
fn census(app: &mut App) -> String {
    let entities = app.world.query::<Entity>().iter(&app.world).count();
    let assets = app
        .world
        .get_resource::<AssetTypes>()
        .map(|t| t.report(&app.world))
        .unwrap_or_default();
    let list: Vec<String> = assets
        .iter()
        .filter(|(_, s)| s.count > 0)
        .map(|(name, s)| format!("{name} {}", s.count))
        .collect();
    format!("{entities} entities; assets: {}", list.join(", "))
}

/// The cells from above: loaded green, out with a proxy amber, out without dark; the camera
/// and its load reach in white.
fn map(path: &Path, layout: &SceneLayout, states: &[(String, CellState, bool)], eye: DVec3) {
    // Half a pixel a meter: world x -150..1950, z -700..500 (north, -z, up).
    let (x0, z0, scale) = (-150.0, -700.0, 0.5);
    let (w, h) = (1050u32, 600u32);
    let px = |x: f64| ((x - x0) * scale) as i64;
    let pz = |z: f64| ((z - z0) * scale) as i64;
    let mut img = image::RgbImage::from_pixel(w, h, image::Rgb([24, 27, 31]));
    let inside = |x: i64, y: i64| (0..i64::from(w)).contains(&x) && (0..i64::from(h)).contains(&y);
    // Fine cells filled; cells of coarser levels (the castle's) outlined over them, 3 px.
    let mut cells: Vec<&PlannedCell> = layout.cells.iter().collect();
    cells.sort_by_key(|c| c.level);
    for c in cells {
        let (state, proxy) = states
            .iter()
            .find(|s| s.0 == c.key)
            .map(|s| (s.1, s.2))
            .unwrap_or((CellState::Unloaded, false));
        let color = match (state, proxy) {
            (CellState::Loaded, _) => [86, 170, 92],
            (_, true) => [196, 146, 58],
            _ => [52, 56, 64],
        };
        let (ax, az) = (c.x as f64 * c.size, c.z as f64 * c.size);
        let (left, top, right, bottom) = (px(ax), pz(az), px(ax + c.size), pz(az + c.size));
        let border = if c.level == 0 { 1 } else { 3 };
        for y in top..bottom {
            for x in left..right {
                let edge = x < left + border
                    || y < top + border
                    || x >= right - border
                    || y >= bottom - border;
                if !inside(x, y) || (c.level > 0 && !edge) {
                    continue;
                }
                let rgb = match (edge, c.level) {
                    (true, 0) => [12, 12, 12],
                    (true, _) => color.map(|v: u8| v.saturating_add(50)),
                    _ => color,
                };
                img.put_pixel(x as u32, y as u32, image::Rgb(rgb));
            }
        }
    }
    let (ex, ez) = (px(eye.x), pz(eye.z));
    let r = 256.0 * scale;
    for i in 0..720 {
        let a = f64::from(i) / 720.0 * std::f64::consts::TAU;
        let (x, y) = (ex + (a.cos() * r) as i64, ez + (a.sin() * r) as i64);
        if inside(x, y) {
            img.put_pixel(x as u32, y as u32, image::Rgb([240, 240, 240]));
        }
    }
    for y in ez - 4..=ez + 4 {
        for x in ex - 4..=ex + 4 {
            if inside(x, y) {
                img.put_pixel(x as u32, y as u32, image::Rgb([255, 255, 255]));
            }
        }
    }
    img.save(path).expect("map png");
    println!("{}", path.display());
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let out = PathBuf::from(
        args.get(1)
            .cloned()
            .unwrap_or_else(|| "scene_stream_shots".into()),
    );
    let (w, h) = args
        .get(2)
        .and_then(|s| s.split_once('x'))
        .and_then(|(a, b)| Some((a.parse().ok()?, b.parse().ok()?)))
        .unwrap_or((1280u32, 720u32));
    std::fs::create_dir_all(&out).expect("create output dir");
    let cache = out.join("cache");
    let doc = valley();
    println!(
        "scene: {} roots, {} KB of JSON",
        doc.entities.len(),
        doc.to_json_pretty().len() / 1024
    );
    bake(&doc, &cache);

    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut win = app.world.resource_mut::<Window>();
        win.width = w;
        win.height = h;
    }
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 30.0,
        });
    app.add_plugins((RenderPlugin, engine_world::WorldPlugin, StreamingPlugin));
    {
        let mut s = app.world.resource_mut::<HlodSettings>();
        s.settle_frames = 3;
        s.cache_dir = cache.clone();
    }
    // Hard swaps: lavapipe frames take seconds, longer than any fade.
    app.world.resource_mut::<StreamingBudget>().fade_s = 0.0;
    let eye = DVec3::new(-90.0, 70.0, 150.0);
    app.world.spawn((
        Camera {
            exposure: Exposure::Manual { ev100: 14.2 },
            ..Camera::perspective(40.0)
        },
        Transform::from_translation(eye).looking_at(DVec3::new(700.0, 0.0, -120.0), Vec3::Y),
        StreamingSource::default(),
    ));
    let layout =
        stream_scene(&mut app.world, "valley", &doc, &ScenePartition::default()).expect("scene");
    println!(
        "partition: {} always in, {} cells ({} on coarser levels)",
        layout.persistent.len(),
        layout.cells.len(),
        layout.cells.iter().filter(|c| c.level > 0).count()
    );
    let settle = |app: &mut App| {
        for _ in 0..60 {
            app.update();
            let cells = app.world.resource::<StreamedCells>();
            if cells.waking().count() == 0
                && cells.cells.iter().all(|c| c.state != CellState::Unloading)
            {
                break;
            }
        }
        app.update_n(3);
        if let Some(d) = app.world.get_resource::<RenderDisabled>() {
            eprintln!("no GPU: {}", d.0);
            std::process::exit(0);
        }
    };
    let shot = |app: &mut App, name: &str| {
        let path = out.join(format!("{name}.png"));
        match capture_now(&mut app.world, &path) {
            Ok(_) => println!("{}", path.display()),
            Err(e) => eprintln!("{name}: {e}"),
        }
    };
    let states = |app: &mut App| -> Vec<(String, CellState, bool)> {
        let showing: Vec<bool> = app
            .world
            .resource::<Proxies>()
            .triangles()
            .iter()
            .map(|t| *t > 0)
            .collect();
        app.world
            .resource::<StreamedCells>()
            .cells
            .iter()
            .enumerate()
            .map(|(i, c)| {
                (
                    c.key.clone(),
                    c.state,
                    showing.get(i).copied().unwrap_or(false),
                )
            })
            .collect()
    };
    settle(&mut app);
    let s = states(&mut app);
    println!(
        "streamed: {} of {} cells in; {}",
        s.iter().filter(|c| c.1 == CellState::Loaded).count(),
        s.len(),
        census(&mut app)
    );
    shot(&mut app, "streamed");
    map(&out.join("map.png"), &layout, &s, eye);
    let keys: Vec<String> = s.iter().map(|c| c.0.clone()).collect();
    for key in &keys {
        app.world
            .resource_mut::<StreamedCells>()
            .pins
            .insert(key.clone(), true);
    }
    settle(&mut app);
    println!("all loaded: {}", census(&mut app));
    {
        let cells = app.world.resource::<StreamedCells>();
        let secs: f32 = cells.cells.iter().map(|c| c.load_secs).sum();
        let entities: u32 = cells.cells.iter().map(|c| c.entities).sum();
        let worst = cells.cells.iter().map(|c| c.load_secs).fold(0.0, f32::max);
        println!(
            "spawning: {:.1} ms for {entities} entities ({:.1} µs each), slowest cell {:.1} ms",
            secs * 1000.0,
            secs * 1e6 / entities.max(1) as f32,
            worst * 1000.0
        );
    }
    shot(&mut app, "all_loaded");
    // Down the road by the castle: the west end goes, the east end comes.
    app.world.resource_mut::<StreamedCells>().pins.clear();
    let eye = DVec3::new(1150.0, 60.0, 60.0);
    {
        let mut q = app.world.query_filtered::<&mut Transform, With<Camera>>();
        for mut t in q.iter_mut(&mut app.world) {
            *t = Transform::from_translation(eye)
                .looking_at(DVec3::new(1450.0, 10.0, -420.0), Vec3::Y);
        }
    }
    settle(&mut app);
    let s = states(&mut app);
    println!(
        "moved: {} of {} cells in; {}",
        s.iter().filter(|c| c.1 == CellState::Loaded).count(),
        s.len(),
        census(&mut app)
    );
    shot(&mut app, "moved");
    map(&out.join("map_moved.png"), &layout, &s, eye);
    // From the hills 2 km west, every cell is out: first each cell's own proxy, then the far
    // groups (HLOD1) standing in for every out cell of their kilometre.
    let eye = DVec3::new(-1900.0, 320.0, 650.0);
    {
        let mut q = app.world.query::<(&mut Transform, &mut Camera)>();
        for (mut t, mut camera) in q.iter_mut(&mut app.world) {
            *t = Transform::from_translation(eye)
                .looking_at(DVec3::new(750.0, 0.0, -150.0), Vec3::Y);
            // A long lens, as from a lookout.
            *camera = Camera {
                exposure: Exposure::Manual { ev100: 14.2 },
                ..Camera::perspective(13.0)
            };
        }
    }
    let far = |app: &mut App, name: &str, groups: bool| {
        app.world.resource_mut::<HlodSettings>().group_from_m =
            if groups { 1500.0 } else { f64::INFINITY };
        settle(app);
        // Group fades take wall time; lavapipe frames are slow enough for it.
        app.update_n(2);
        let cells = app
            .world
            .query_filtered::<&MeshRenderer, With<HlodProxy>>()
            .iter(&app.world)
            .count();
        let groups = app
            .world
            .query_filtered::<&MeshRenderer, With<HlodGroupProxy>>()
            .iter(&app.world)
            .count();
        let drawn: Vec<engine::assets::Handle<Mesh>> = app
            .world
            .query::<(&MeshRenderer, Option<&HlodProxy>, Option<&HlodGroupProxy>)>()
            .iter(&app.world)
            .filter(|(_, a, b)| a.is_some() || b.is_some())
            .filter_map(|(r, _, _)| match &r.mesh {
                MeshRef::Handle(h) => Some(*h),
                _ => None,
            })
            .collect();
        let meshes = app.world.resource::<engine::assets::Assets<Mesh>>();
        let tris: usize = drawn
            .iter()
            .filter_map(|h| meshes.get(*h).map(|m| m.indices.len() / 3))
            .sum();
        println!("{name}: {cells} cell proxies + {groups} group proxies drawn, {tris} triangles");
        shot(app, name);
    };
    far(&mut app, "far_cells", false);
    far(&mut app, "far_groups", true);
}
