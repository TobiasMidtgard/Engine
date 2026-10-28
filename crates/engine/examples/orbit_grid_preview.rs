//! A procedural, vertical streaming grid riding a moving moon ([`engine::stream_grid`]),
//! shown headless: a debris belt of 400 m cube cells around a 1.2 km moon, generated cell by
//! cell from a hash as a ship comes within 600 m (rocks, now and then a station), nothing of it
//! authored or registered up front. The ship is a streaming source parked on the moon's frame;
//! the camera is not one, so it can look from outside. Shots:
//!
//! - `ship.png`: from the ship, in the belt;
//! - `outside.png`: from 7 km out, the bubble of cells around the ship;
//! - `outside_moved.png`: the moon (and the ship on it) 4 km on and turned 30°: the same
//!   cells, carried along, none made again;
//! - `outside_trip.png`: after a quarter orbit, the bubble with the ship, the cells behind
//!   retired;
//! - `map.png`: the belt from above, columns of cells made at the start (blue) and after the
//!   trip (green), the ship's path in white;
//!
//! and prints cells made, retired, alive and the slots they use.
//!
//! `cargo run -p engine --example orbit_grid_preview -- <out dir> [WxH]`

use std::f64::consts::TAU;
use std::path::{Path, PathBuf};

use engine::stream::{StreamingBudget, StreamingPlugin};
use engine::stream_grid::{CellPlace, GridCell, StreamGrid, StreamGrids, StreamGridsExt, once};
use engine_core::math::{DVec3, Quat, Vec3};
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, capture_now};
use engine_world::{StreamFrame, StreamedCells, StreamingSource};

const MOON_R: f64 = 1200.0;
const CELL_M: f64 = 400.0;
const LOAD_M: f64 = 600.0;
/// The belt: a flat ring around the moon's equator.
const BELT: (f64, f64, f64) = (1700.0, 3300.0, 450.0);

/// A deterministic number in [0, 1) from a seed and an index.
fn unit(seed: u64, i: u64) -> f64 {
    let mut h = seed ^ i.wrapping_mul(0x9e37_79b9_7f4a_7c15);
    h ^= h >> 31;
    h = h.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    h ^= h >> 29;
    (h >> 11) as f64 / (1u64 << 53) as f64
}

fn in_belt(p: DVec3) -> bool {
    let r = (p.x * p.x + p.z * p.z).sqrt();
    r > BELT.0 && r < BELT.1 && p.y.abs() < BELT.2
}

fn rock_material(shade: f64) -> StandardMaterial {
    let v = (0.16 + shade * 0.14) as f32;
    StandardMaterial::color(v * 1.05, v, v * 0.92).with_roughness(0.92)
}

/// Spawns `mesh` as a child of the moon at `local` (from the cell's center).
fn part(world: &mut World, place: &CellPlace, name: &str, local: Transform, mesh: MeshRenderer) {
    let frame = place.frame.expect("the belt rides the moon");
    world.spawn((
        Name::new(name.to_string()),
        place.in_frame(local),
        mesh,
        ChildOf(frame),
    ));
}

fn rocks(world: &mut World, place: &CellPlace, seed: u64) {
    let n = 5 + (unit(seed, 0) * 9.0) as u64;
    for i in 0..n {
        let k = 10 + i * 8;
        let at = DVec3::new(
            unit(seed, k) - 0.5,
            (unit(seed, k + 1) - 0.5) * 0.6,
            unit(seed, k + 2) - 0.5,
        ) * CELL_M;
        if !in_belt(place.local_center + at) {
            continue;
        }
        let size = 6.0 + unit(seed, k + 3).powi(3) * 70.0;
        let stretch = Vec3::new(
            1.0 + unit(seed, k + 4) as f32 * 0.8,
            0.6 + unit(seed, k + 5) as f32 * 0.5,
            0.8 + unit(seed, k + 6) as f32 * 0.6,
        );
        let turn = Quat::from_rotation_x(unit(seed, k + 7) as f32 * 6.3)
            * Quat::from_rotation_y(unit(seed, k + 1) as f32 * 6.3);
        part(
            world,
            place,
            "rock",
            Transform::from_translation(at)
                .with_rotation(turn)
                .with_scale(stretch),
            MeshRenderer::new(
                Shape::Sphere {
                    radius: size as f32,
                    segments: 7,
                },
                rock_material(unit(seed, k + 2)),
            ),
        );
    }
}

fn station(world: &mut World, place: &CellPlace, seed: u64) {
    let hull = StandardMaterial::hex(0xc9ccd1)
        .with_metallic(0.7)
        .with_roughness(0.35);
    let panel = StandardMaterial::hex(0x1d2a4a)
        .with_metallic(0.4)
        .with_roughness(0.25);
    let lit = StandardMaterial::hex(0xfff1c8).with_emissive([1.0, 0.85, 0.55], 8.0);
    let tilt = Quat::from_rotation_z((unit(seed, 1) as f32 - 0.5) * 0.6);
    let at = |v: DVec3| Transform::from_translation(tilt.as_dquat() * v);
    part(
        world,
        place,
        "station ring",
        at(DVec3::ZERO).with_rotation(tilt),
        MeshRenderer::new(
            Shape::Torus {
                radius: 70.0,
                tube: 7.0,
                segments: 48,
            },
            hull,
        ),
    );
    part(
        world,
        place,
        "station hub",
        at(DVec3::ZERO).with_rotation(tilt),
        MeshRenderer::new(
            Shape::Cylinder {
                radius: 14.0,
                height: 120.0,
                segments: 24,
            },
            hull,
        ),
    );
    for i in 0..4 {
        let a = f64::from(i) * TAU / 4.0;
        let d = DVec3::new(a.cos(), 0.0, a.sin());
        part(
            world,
            place,
            "station spoke",
            at(d * 35.0)
                .with_rotation(tilt * Quat::from_rotation_y(-a as f32))
                .with_scale(Vec3::ONE),
            MeshRenderer::new(
                Shape::Cuboid {
                    size: [70.0, 3.0, 3.0],
                },
                hull,
            ),
        );
    }
    for side in [-1.0, 1.0] {
        part(
            world,
            place,
            "station panel",
            at(DVec3::new(0.0, side * 75.0, 0.0)).with_rotation(tilt),
            MeshRenderer::new(
                Shape::Cuboid {
                    size: [150.0, 1.0, 30.0],
                },
                panel,
            ),
        );
        part(
            world,
            place,
            "station light",
            at(DVec3::new(0.0, side * 62.0, 0.0)),
            MeshRenderer::new(
                Shape::Sphere {
                    radius: 3.0,
                    segments: 8,
                },
                lit,
            ),
        );
    }
}

/// What a belt cell holds: nothing off the belt, else rocks, one cell in 25 a station too.
fn make(cell: &GridCell) -> Option<engine::stream_grid::GridSpawn> {
    let grid = StreamGrid::new("belt", CELL_M, LOAD_M).vertical();
    if !in_belt(grid.local_center(cell)) {
        return None;
    }
    let seed = cell.seed(7);
    let has_station = unit(seed, 99) < 0.04;
    Some(once(move |world, place| {
        rocks(world, place, seed);
        if has_station {
            station(world, place, seed);
        }
    }))
}

fn moon_pose(at: DVec3, turn_deg: f64) -> (Transform, GlobalTransform) {
    let t = Transform::from_translation(at)
        .with_rotation(Quat::from_rotation_y(turn_deg.to_radians() as f32));
    (t, GlobalTransform::from(t))
}

/// The ship's place on the moon's frame, `a` radians round the belt.
fn ship_local(a: f64) -> DVec3 {
    DVec3::new(a.cos() * 2500.0, 60.0, a.sin() * 2500.0)
}

/// The belt from above in the moon's frame: cells made at the start in blue, after the trip
/// in green (both: teal), the ship's path in white.
fn map(path: &Path, start: &[GridCell], end: &[GridCell], trail: &[DVec3]) {
    let scale = 0.08; // pixels a meter: ±3.6 km
    let (w, h) = (600u32, 600u32);
    let to_px = |x: f64, z: f64| {
        (
            (x * scale + f64::from(w) / 2.0) as i64,
            (z * scale + f64::from(h) / 2.0) as i64,
        )
    };
    let mut img = image::RgbImage::from_pixel(w, h, image::Rgb([14, 16, 22]));
    let put = |img: &mut image::RgbImage, x: i64, y: i64, c: [u8; 3]| {
        if (0..i64::from(w)).contains(&x) && (0..i64::from(h)).contains(&y) {
            img.put_pixel(x as u32, y as u32, image::Rgb(c));
        }
    };
    // The moon and the belt's edges.
    for y in 0..h {
        for x in 0..w {
            let (mx, mz) = (
                (f64::from(x) - f64::from(w) / 2.0) / scale,
                (f64::from(y) - f64::from(h) / 2.0) / scale,
            );
            let r = (mx * mx + mz * mz).sqrt();
            if r < MOON_R {
                put(&mut img, x.into(), y.into(), [70, 70, 76]);
            } else if (r - BELT.0).abs() < 12.0 || (r - BELT.1).abs() < 12.0 {
                put(&mut img, x.into(), y.into(), [44, 48, 60]);
            }
        }
    }
    let grid = StreamGrid::new("belt", CELL_M, LOAD_M).vertical();
    let columns = |cells: &[GridCell]| -> Vec<(i64, i64)> {
        let mut c: Vec<(i64, i64)> = cells.iter().map(|c| (c.x, c.z)).collect();
        c.sort_unstable();
        c.dedup();
        c
    };
    let (a, b) = (columns(start), columns(end));
    let mut all = a.clone();
    all.extend(b.iter().copied());
    all.sort_unstable();
    all.dedup();
    for (cx, cz) in all {
        let color = match (a.contains(&(cx, cz)), b.contains(&(cx, cz))) {
            (true, true) => [60, 170, 160],
            (true, false) => [70, 110, 200],
            _ => [90, 180, 90],
        };
        let c = grid.local_center(&GridCell { x: cx, y: 0, z: cz });
        let (x0, y0) = to_px(c.x - CELL_M / 2.0, c.z - CELL_M / 2.0);
        let (x1, y1) = to_px(c.x + CELL_M / 2.0, c.z + CELL_M / 2.0);
        for y in y0 + 1..y1 {
            for x in x0 + 1..x1 {
                put(&mut img, x, y, color);
            }
        }
    }
    for p in trail {
        let (x, y) = to_px(p.x, p.z);
        for dy in -1..=1 {
            for dx in -1..=1 {
                put(&mut img, x + dx, y + dy, [250, 250, 250]);
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
            .unwrap_or_else(|| "orbit_grid_shots".into()),
    );
    let (w, h) = args
        .get(2)
        .and_then(|s| s.split_once('x'))
        .and_then(|(a, b)| Some((a.parse().ok()?, b.parse().ok()?)))
        .unwrap_or((1280u32, 720u32));
    std::fs::create_dir_all(&out).expect("create output dir");

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
        let mut budget = app.world.resource_mut::<StreamingBudget>();
        budget.fade_s = 0.0;
        budget.loads_per_frame = 16;
    }
    // In space: the sky's planet (Earth by default) 40 000 km below, so the sky is black.
    app.world
        .resource_mut::<engine_render::Atmosphere>()
        .planet_center = DVec3::new(0.0, -4.0e7, 2.0e7);
    app.world.spawn((
        Transform::default().looking_at(DVec3::new(-0.8, -0.25, -0.4), Vec3::Y),
        DirectionalLight {
            color: [1.0, 0.95, 0.88],
            illuminance: 60_000.0,
            ..Default::default()
        },
    ));
    let moon = app
        .world
        .spawn((
            Name::new("moon"),
            StreamFrame("moon".into()),
            moon_pose(DVec3::ZERO, 0.0),
            MeshRenderer::new(
                Shape::Sphere {
                    radius: MOON_R as f32,
                    segments: 96,
                },
                StandardMaterial::preset(MaterialPreset::Rock).with_triplanar(),
            ),
        ))
        .id();
    // The ship rides the moon's frame (a child of it), so where it is in the moon's space is
    // where the belt streams.
    let mut a = 0.0;
    let ship = app
        .world
        .spawn((
            Name::new("ship"),
            Transform::from_translation(ship_local(a)),
            GlobalTransform::default(),
            StreamingSource {
                lookahead_s: 0.0,
                ..Default::default()
            },
            ChildOf(moon),
        ))
        .id();
    let camera = app
        .world
        .spawn((
            Camera {
                exposure: Exposure::Manual { ev100: 13.6 },
                ..Camera::perspective(55.0)
            },
            Transform::default(),
        ))
        .id();
    app.world.stream_grid(
        StreamGrid::new("belt", CELL_M, LOAD_M)
            .vertical()
            .on_frame("moon"),
        make,
    );

    let settle = |app: &mut App| {
        for _ in 0..40 {
            app.update();
            if app.world.resource::<StreamedCells>().waking().count() == 0 {
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
    let stats = |app: &mut App, label: &str| {
        let g = app.world.resource::<StreamGrids>().report().remove(0);
        let (live, slots) = {
            let cells = app.world.resource::<StreamedCells>();
            (
                cells.cells.iter().filter(|c| !c.retired).count(),
                cells.cells.len(),
            )
        };
        let rocks = app
            .world
            .query::<&Name>()
            .iter(&app.world)
            .filter(|n| n.as_str() == "rock")
            .count();
        let stations = app
            .world
            .query::<&Name>()
            .iter(&app.world)
            .filter(|n| n.as_str() == "station ring")
            .count();
        println!(
            "{label}: {} cells made, {} retired, {} known around the ship ({} with content); \
             {live} live cells in {slots} slots; {rocks} rocks, {stations} stations",
            g.made, g.retired, g.known, g.cells
        );
    };
    let made_cells = |app: &mut App| -> Vec<GridCell> {
        app.world
            .resource::<StreamedCells>()
            .cells
            .iter()
            .filter(|c| !c.retired)
            .filter_map(|c| {
                let xyz = c.key.strip_prefix("grid:belt/")?;
                let v: Vec<i64> = xyz.split(',').filter_map(|s| s.parse().ok()).collect();
                (v.len() == 3).then(|| GridCell {
                    x: v[0],
                    y: v[1],
                    z: v[2],
                })
            })
            .collect()
    };
    let look = |app: &mut App, from: DVec3, to: DVec3| {
        app.world
            .entity_mut(camera)
            .insert(Transform::from_translation(from).looking_at(to, Vec3::Y));
    };
    let moon_world = |app: &mut App, local: DVec3| -> DVec3 {
        let t = app
            .world
            .get::<Transform>(moon)
            .copied()
            .unwrap_or_default();
        t.translation + t.rotation.as_dquat() * local
    };

    settle(&mut app);
    let start = made_cells(&mut app);
    stats(&mut app, "start");
    // From the ship, toward the nearest station if one is in, else along the belt.
    let eye = moon_world(&mut app, ship_local(a) + DVec3::new(0.0, 20.0, 0.0));
    let ahead = app
        .world
        .query::<(&Name, &GlobalTransform)>()
        .iter(&app.world)
        .filter(|(n, _)| n.as_str() == "station ring")
        .map(|(_, g)| g.translation())
        .min_by(|a, b| a.distance(eye).total_cmp(&b.distance(eye)))
        .filter(|p| p.distance(eye) < 1200.0);
    let ahead = match ahead {
        Some(p) => p,
        None => moon_world(&mut app, ship_local(a + 0.25) + DVec3::new(0.0, -80.0, 0.0)),
    };
    // A little back from the station, so it fits.
    let eye = ahead + (eye - ahead).normalize() * (eye.distance(ahead).min(420.0));
    look(&mut app, eye, ahead);
    app.update_n(2);
    shot(&mut app, "ship");
    // From 7 km out.
    let outside = |app: &mut App, name: &str| {
        let from = moon_world(app, DVec3::new(4200.0, 3200.0, 5200.0));
        let to = moon_world(app, DVec3::new(1400.0, 0.0, 600.0));
        look(app, from, to);
        app.update_n(2);
        shot(app, name);
    };
    outside(&mut app, "outside");
    // The moon moves on and turns; the ship rides it.
    app.world
        .entity_mut(moon)
        .insert(moon_pose(DVec3::new(4000.0, 0.0, -1500.0), 30.0));
    settle(&mut app);
    stats(&mut app, "moon moved 4.3 km and turned 30°");
    outside(&mut app, "outside_moved");
    // A quarter orbit round the belt, 100 m a step.
    let mut trail = vec![ship_local(a)];
    let end_a = TAU / 4.0;
    let steps = ((end_a - a) * 2500.0 / 100.0) as usize;
    for _ in 0..steps {
        a += end_a / steps as f64;
        app.world
            .entity_mut(ship)
            .insert(Transform::from_translation(ship_local(a)));
        app.update_n(4);
        trail.push(ship_local(a));
    }
    settle(&mut app);
    app.update_n(20);
    stats(&mut app, "after a quarter orbit");
    let end = made_cells(&mut app);
    outside(&mut app, "outside_trip");
    map(&out.join("map.png"), &start, &end, &trail);
}
