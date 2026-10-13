//! Look-development renders of the showcase ground and scenery, headless:
//!
//! ```sh
//! cargo run -p engine_foliage --example lookdev_ground --release -- /tmp/lookdev_ground [shot…]
//! ```
//!
//! Grows the showcase forest and mushroom cave (as `foliage_shots` does), then photographs the
//! ground close up: the trail, the forest floor under oaks and under conifers, the pond shore,
//! the burn scar, the rocks on the dry ridge and the cave's mineral pool. Name shots after the
//! output directory to render only those. `LOOKDEV_SIZE=1280x720` changes the image size
//! (default 640×360).

use engine_assets::Handle;
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_foliage::FoliagePlugin;
use engine_foliage::{demo, scenery};
use engine_render::prelude::*;
use engine_render::{Exposure, SkyLightVolume};
use engine_world::WeatherState;

const FOREST: DVec3 = DVec3::new(0.0, 0.0, 0.0);
const CAVE: DVec3 = DVec3::new(400.0, 0.0, 0.0);
/// A bare patch of forest floor to lay out rocks and debris on.
const LAB: DVec3 = DVec3::new(0.0, 0.0, 300.0);
const STEP: f64 = 1.0 / 15.0;

/// A close-up of the ground: name, eye and target (local to the zone), hour, cave or forest.
struct Shot {
    name: &'static str,
    eye: [f32; 3],
    target: [f32; 3],
    hour: f32,
    cave: bool,
}

const SHOTS: &[Shot] = &[
    Shot {
        name: "trail",
        eye: [71.0, 1.6, 35.4],
        target: [60.0, 0.0, 32.0],
        hour: 9.5,
        cave: false,
    },
    Shot {
        name: "floor_oaks",
        eye: [6.0, 1.5, -24.0],
        target: [0.0, 0.0, -30.0],
        hour: 10.5,
        cave: false,
    },
    Shot {
        name: "floor_conifers",
        eye: [-30.0, 1.5, -12.0],
        target: [-36.0, 0.0, -18.0],
        hour: 10.5,
        cave: false,
    },
    Shot {
        name: "pond",
        eye: [-6.0, 1.9, 17.8],
        target: [-10.0, 0.0, 10.0],
        hour: 10.0,
        cave: false,
    },
    Shot {
        name: "burn_scar",
        eye: [-26.0, 1.8, 38.0],
        target: [-33.0, 0.0, 47.0],
        hour: 10.5,
        cave: false,
    },
    Shot {
        name: "ridge_rocks",
        eye: [-50.5, 1.8, -20.0],
        target: [-55.0, 0.2, -32.0],
        hour: 16.0,
        cave: false,
    },
    Shot {
        name: "cave_pool",
        eye: [0.5, 2.4, -4.0],
        target: [0.0, 0.0, 2.5],
        hour: 14.0,
        cave: true,
    },
];

fn sun_direction(hour: f32) -> Vec3 {
    let theta = (hour - 12.0) / 24.0 * std::f32::consts::TAU;
    Vec3::new(theta.sin() * 0.9, theta.cos() * 0.8, -0.35).normalize()
}

fn set_hour(world: &mut World, hour: f32) {
    let dir = sun_direction(hour);
    world.resource_mut::<WeatherState>().sun_direction = dir;
    world.resource_mut::<WeatherState>().time_of_day_h = hour;
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

/// A simple cave around the cave demo (the showcase builds its own).
fn cave_walls(commands: &mut Commands, c: DVec3) {
    let rock = StandardMaterial::hex(0x3e3b39).with_roughness(0.9);
    let (w, h, d) = (30.0f32, 9.0f32, 34.0f32);
    let mut wall = |size: [f32; 3], at: DVec3| {
        commands.spawn((
            Transform::from_translation(at),
            MeshRenderer::new(Shape::Cuboid { size }, rock),
        ));
    };
    wall(
        [w, h, 2.0],
        c + DVec3::new(0.0, h as f64 * 0.5, d as f64 * 0.5),
    );
    wall(
        [2.0, h, d],
        c + DVec3::new(-w as f64 * 0.5, h as f64 * 0.5, 0.0),
    );
    wall(
        [2.0, h, d],
        c + DVec3::new(w as f64 * 0.5, h as f64 * 0.5, 0.0),
    );
    wall(
        [w + 24.0, 5.0, d + 16.0],
        c + DVec3::new(0.0, h as f64 + 2.5, 4.0),
    );
    for x in [-1.0, 1.0] {
        wall(
            [10.0, h, 2.0],
            c + DVec3::new(x * 10.0, h as f64 * 0.5, -d as f64 * 0.5),
        );
    }
    commands.spawn((
        Transform::from_translation(c + DVec3::new(0.0, h as f64 * 0.5, 0.0)),
        SkyLightVolume::new([w * 0.5 - 1.0, h * 0.5, d * 0.5 - 1.0])
            .open(BoxFace::NegZ)
            .with_fade(12.0),
    ));
}

/// Write each painted ground layer as two images: albedo (RGB) and height (grey) side by side,
/// then normal (RG), roughness and occlusion.
fn save_layers(out: &str) {
    let n = scenery::LAYER_SIZE;
    for layer in scenery::GroundLayer::ALL {
        let t = scenery::paint_layer(layer, n);
        let mut sheet = vec![0u8; n * 2 * n * 3 * 2];
        let w = n * 2;
        for y in 0..n {
            for x in 0..n {
                let i = (y * n + x) * 4;
                let a = &t.albedo_height[i..i + 4];
                let m = &t.normal_rough_ao[i..i + 4];
                let mut put = |px: usize, py: usize, c: [u8; 3]| {
                    let o = (py * w + px) * 3;
                    sheet[o..o + 3].copy_from_slice(&c);
                };
                put(x, y, [a[0], a[1], a[2]]);
                put(x + n, y, [a[3]; 3]);
                put(x, y + n, [m[0], m[1], 255]);
                put(x + n, y + n, [m[2], m[3], m[3]]);
            }
        }
        let path = format!("{out}/layer_{layer:?}.png").to_lowercase();
        image::save_buffer(&path, &sheet, w as u32, w as u32, image::ColorType::Rgb8)
            .expect("save layer");
        println!("saved {path}");
    }
}

/// Every debris mesh in rows on the lab patch: twigs, branches, cones, leaf clumps, reeds.
fn lay_out_debris(world: &mut World) {
    let Some(a) = world.get_resource::<scenery::SceneryAssets>().cloned() else {
        return;
    };
    // Meshes, their material, the row's z and the spacing along x.
    type Row<'a> = (&'a [Handle<Mesh>], &'a Handle<StandardMaterial>, f32, f32);
    let rows: [Row; 6] = [
        (&a.twigs, &a.bark, -0.5, 0.6),
        (&a.branches, &a.bark, -1.6, 1.9),
        (&a.pine_cones, &a.cones, 0.0, 0.3),
        (&a.spruce_cones, &a.cones, 0.0, 0.3),
        (&a.leaf_clumps, &a.leaves, 0.6, 0.55),
        (&a.reeds, &a.reed, 3.0, 0.9),
    ];
    let mut x0 = -4.0;
    let mut batch = Vec::new();
    for (meshes, material, z, step) in rows {
        let mut x = if z == 0.0 { x0 } else { -4.0 };
        for m in meshes {
            batch.push((
                Transform::from_translation(LAB + DVec3::new(x as f64, 0.045, z as f64))
                    .with_rotation(Quat::from_rotation_y(0.3)),
                MeshRenderer::new(*m, *material),
            ));
            x += step;
        }
        if z == 0.0 {
            x0 = x + 0.2;
        }
    }
    world.spawn_batch(batch);
}

/// Debris pieces shown (not culled by distance) and their triangles before frustum culling.
fn debris_load(world: &mut World) -> (usize, usize) {
    let mut q = world.query_filtered::<&MeshRenderer, With<scenery::Debris>>();
    let handles: Vec<_> = q
        .iter(world)
        .filter(|r| r.visible)
        .filter_map(|r| match &r.mesh {
            MeshRef::Handle(h) => Some(*h),
            _ => None,
        })
        .collect();
    let meshes = world.resource::<engine_assets::Assets<Mesh>>();
    let tris = handles
        .iter()
        .filter_map(|h| meshes.get(*h))
        .map(|m| m.indices.len() / 3)
        .sum();
    (handles.len(), tris)
}

fn main() {
    engine_core::log::init("info,wgpu=warn,naga=warn");
    let mut args = std::env::args().skip(1);
    let out = args.next().unwrap_or_else(|| "/tmp/lookdev_ground".into());
    let only: Vec<String> = args.collect();
    std::fs::create_dir_all(&out).expect("output directory");
    let (width, height) = std::env::var("LOOKDEV_SIZE")
        .ok()
        .and_then(|s| {
            let (w, h) = s.split_once('x')?;
            Some((w.parse().ok()?, h.parse().ok()?))
        })
        .unwrap_or((640u32, 360u32));
    if only.iter().any(|n| n == "layers") {
        save_layers(&out);
        if only.len() == 1 {
            return;
        }
    }

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

    app.world.resource_mut::<Atmosphere>().ground_color = [0.09, 0.18, 0.04];
    app.world.spawn((
        Transform::default(),
        MeshRenderer::new(
            Shape::Plane {
                size: [4000.0, 4000.0],
                subdivisions: 1,
            },
            StandardMaterial::hex(0x55743a).with_roughness(0.95),
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
                fov_y_deg: 55.0,
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
    {
        let mut commands = app.world.commands();
        demo::spawn_demo(&mut commands, FOREST);
        demo::spawn_cave_demo(&mut commands, CAVE);
        cave_walls(&mut commands, CAVE);
        // The lab: a patch of bare forest floor to lay every piece of scenery out on.
        commands.spawn((
            Transform::from_translation(LAB),
            demo::GroundCover {
                half_extents: [5.0, 4.0],
                round: false,
                cell_m: 0.25,
                debris: 0.0,
                ..Default::default()
            },
        ));
        let mut rocks = Vec::new();
        for shape in 0..scenery::ROCK_SHAPES {
            let x = -4.0 + shape as f32 * 0.72;
            let w = if scenery::rock_is_rounded(shape) {
                0.32
            } else {
                0.5
            };
            rocks.push(scenery::PlacedRock {
                pos: Vec2::new(x, 1.6),
                rotation: Quat::from_rotation_y(shape as f32 * 0.7),
                size: Vec3::new(w, w * 0.6, w * 0.8),
                rock: scenery::Rock {
                    shape,
                    bury_m: w * 0.12,
                    moss: if shape % 3 == 0 { 0.8 } else { 0.25 },
                    lichen: 0.5,
                    ..Default::default()
                },
            });
        }
        let mut rng = Rng::new(9);
        scenery::rock_cluster(
            &mut rng,
            Vec2::new(3.2, 2.9),
            1.5,
            0.5,
            [1.0; 3],
            &mut rocks,
        );
        scenery::spawn_rocks(&mut commands, LAB, rocks);
    }
    app.world.flush();
    let mut felling = app.world.query::<&mut demo::DemoFelling>();
    for mut d in felling.iter_mut(&mut app.world) {
        d.max_trees = 0;
    }
    // Freeze the ecology: these are stills of the ground.
    app.world
        .resource_mut::<engine_foliage::EcologySettings>()
        .years_per_second = 0.0;
    let started = std::time::Instant::now();
    app.update();
    println!(
        "grew the forest and cave in {:.1} s",
        started.elapsed().as_secs_f32()
    );
    lay_out_debris(&mut app.world);

    // A close look at the biggest outcrop on the ridge (well inside the forest).
    let outcrop = demo::forest_rocks()
        .into_iter()
        .filter(|r| {
            r.pos.length() < 66.0
                && r.pos.distance(Vec2::new(demo::RIDGE[0], demo::RIDGE[1])) < demo::RIDGE[2]
        })
        .max_by(|a, b| a.radius().total_cmp(&b.radius()))
        .map(|r| r.pos)
        .unwrap_or_default();
    // And at the ground where the most debris lies, off the trail.
    let spots: Vec<Vec2> = app
        .world
        .query_filtered::<&Transform, With<scenery::Debris>>()
        .iter(&app.world)
        .map(|t| {
            Vec2::new(
                (t.translation.x - FOREST.x) as f32,
                (t.translation.z - FOREST.z) as f32,
            )
        })
        .filter(|p| p.length() < 70.0 && demo::polyline_distance(*p, &demo::TRAIL) > 4.0)
        .collect();
    let litter = spots
        .iter()
        .max_by_key(|p| spots.iter().filter(|q| q.distance(**p) < 2.0).count())
        .copied()
        .unwrap_or_default();
    let extra = [
        Shot {
            name: "rock_close",
            eye: [outcrop.x + 3.2, 1.5, outcrop.y + 3.6],
            target: [outcrop.x, 0.5, outcrop.y],
            hour: 16.0,
            cave: false,
        },
        Shot {
            name: "lab",
            eye: [0.0, 2.6, LAB.z as f32 - 4.3],
            target: [0.0, 0.0, LAB.z as f32 + 0.9],
            hour: 10.0,
            cave: false,
        },
        Shot {
            name: "lab_close",
            eye: [-2.6, 1.2, LAB.z as f32 - 2.9],
            target: [-2.6, 0.0, LAB.z as f32 - 0.2],
            hour: 10.0,
            cave: false,
        },
        Shot {
            name: "debris_close",
            eye: [litter.x + 1.6, 1.25, litter.y + 1.9],
            target: [litter.x, 0.0, litter.y],
            hour: 11.0,
            cave: false,
        },
    ];
    for shot in SHOTS.iter().chain(extra.iter()) {
        if !only.is_empty() && !only.iter().any(|n| n == shot.name) {
            continue;
        }
        let center = if shot.cave { CAVE } else { FOREST };
        let at = |v: [f32; 3]| center + DVec3::new(v[0] as f64, v[1] as f64, v[2] as f64);
        aim(&mut app.world, at(shot.eye), at(shot.target));
        set_hour(&mut app.world, shot.hour);
        let t = std::time::Instant::now();
        // Plant visuals refresh their distance-dependent parts every 8 frames.
        app.update_n(9);
        let path = format!("{out}/{}.png", shot.name);
        app.world
            .resource_mut::<Screenshots>()
            .request(path.clone());
        app.update();
        println!("saved {path} ({:.1} s)", t.elapsed().as_secs_f32());
        if let Some(stats) = app.world.get_resource::<engine_render::RenderStats>() {
            println!(
                "  {}: {} draw calls, {} instances, {} triangles",
                shot.name, stats.draw_calls, stats.instances, stats.triangles
            );
        }
        let (debris, debris_tris) = debris_load(&mut app.world);
        println!("  scenery debris shown: {debris} pieces, {debris_tris} triangles");
    }
    let rocks = app.world.query::<&scenery::Rock>().iter(&app.world).count();
    let debris = app
        .world
        .query::<&scenery::Debris>()
        .iter(&app.world)
        .count();
    println!("scenery: {rocks} rocks (with pebbles), {debris} debris and reeds");
}
