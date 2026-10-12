//! Renders every procedural material of the water test area ([`DemoMaterials`]) on a sphere
//! and a rounded cube, without a window, into PNGs: an overview, one picture per half row and
//! close-ups. A legend (`legend.txt`) says which surface sits where.
//!
//! ```text
//! cargo run -p engine_water --example material_preview --release -- [out_dir] [views]
//! ```
//!
//! `views` is a comma-separated subset of `overview`, `row0a` … `row3b` and close-ups named
//! after a surface (`Planks`, `Rock`, …); the default renders the overview, every half row and
//! close-ups of Planks, PaintedWood, Rock, Gelcoat, Concrete, Bark and EndGrain.
//!
//! Every mesh uses UVs in meters, the convention the materials expect: round things wrap a
//! whole number of texture tiles; the log's end caps are centered on uv (0, 0); the rope coil
//! wraps one tile around the rope.

use std::f32::consts::{PI, TAU};
use std::fmt::Write as _;

use engine_assets::Assets;
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, Vertex, capture_now};
use engine_water::demo::materials::{DemoMaterials, Surface, tile_size};
use engine_world::WorldPlugin;

const COLS: usize = 7;
const SPACING_X: f64 = 1.7;
const SPACING_Z: f64 = 2.3;
const CUBE: f32 = 0.6;
const RADIUS: f32 = 0.33;

/// Center of slot `i` on the ground.
fn slot(i: usize) -> DVec3 {
    let (c, r) = (i % COLS, i / COLS);
    DVec3::new(
        (c as f64 - (COLS - 1) as f64 * 0.5) * SPACING_X,
        0.0,
        -(r as f64) * SPACING_Z,
    )
}

/// Round `length` meters to a whole number of `tile`s (at least one).
fn whole_tiles(length: f32, tile: f32) -> f32 {
    (length / tile).round().max(1.0) * tile
}

/// UV sphere with UVs in meters: u around the equator (a whole number of tiles), v from the
/// top pole down.
fn sphere(radius: f32, tile: [f32; 2]) -> Mesh {
    let segments = 64u32;
    let rings = 32u32;
    let around = whole_tiles(TAU * radius, tile[0]);
    let mut m = Mesh::default();
    for r in 0..=rings {
        let v = r as f32 / rings as f32;
        let phi = v * PI;
        for s in 0..=segments {
            let u = s as f32 / segments as f32;
            let theta = u * TAU;
            let n = Vec3::new(phi.sin() * theta.cos(), phi.cos(), -phi.sin() * theta.sin());
            m.vertices.push(Vertex::new(
                n * radius,
                n,
                Vec2::new(u * around, v * PI * radius),
            ));
        }
    }
    let row = segments + 1;
    for r in 0..rings {
        for s in 0..segments {
            let i = r * row + s;
            m.indices
                .extend_from_slice(&[i, i + row, i + 1, i + 1, i + row, i + row + 1]);
        }
    }
    m
}

/// Cube of edge `size` with edges rounded to `bevel`, UVs in meters per face (side faces: v
/// runs down, so wood grain stands vertical).
fn rounded_cube(size: f32, bevel: f32) -> Mesh {
    let h = size * 0.5;
    let k = 28u32;
    let faces: [(Vec3, Vec3, Vec3); 6] = [
        (Vec3::Z, Vec3::X, Vec3::Y),
        (Vec3::NEG_Z, Vec3::NEG_X, Vec3::Y),
        (Vec3::X, Vec3::NEG_Z, Vec3::Y),
        (Vec3::NEG_X, Vec3::Z, Vec3::Y),
        (Vec3::Y, Vec3::X, Vec3::NEG_Z),
        (Vec3::NEG_Y, Vec3::X, Vec3::Z),
    ];
    let mut m = Mesh::default();
    for (n, ua, va) in faces {
        let base = m.vertices.len() as u32;
        for j in 0..=k {
            // Denser toward the edges, where the bevel bends.
            let t = (j as f32 / k as f32 * 2.0 - 1.0) * PI * 0.5;
            let t = t.sin() * h;
            for i in 0..=k {
                let s = (i as f32 / k as f32 * 2.0 - 1.0) * PI * 0.5;
                let s = s.sin() * h;
                let flat = n * h + ua * s + va * t;
                let inner = flat.clamp(Vec3::splat(-(h - bevel)), Vec3::splat(h - bevel));
                let d = (flat - inner).normalize_or(n);
                let p = inner + d * bevel;
                m.vertices.push(Vertex::new(p, d, Vec2::new(s + h, h - t)));
            }
        }
        let row = k + 1;
        for j in 0..k {
            for i in 0..k {
                let a = base + j * row + i;
                m.indices
                    .extend_from_slice(&[a, a + 1, a + row + 1, a, a + row + 1, a + row]);
            }
        }
    }
    m
}

/// A log section along x: bark around (u wraps whole tiles), and the two sawn end caps with
/// UVs = local (z, y) in meters from the axis.
fn log(radius: f32, length: f32, bark_tile: [f32; 2]) -> (Mesh, Mesh) {
    let segments = 64u32;
    let around = whole_tiles(TAU * radius, bark_tile[0]);
    let mut side = Mesh::default();
    for j in 0..=1u32 {
        let x = (j as f32 - 0.5) * length;
        for s in 0..=segments {
            let a = s as f32 / segments as f32 * TAU;
            let n = Vec3::new(0.0, a.sin(), a.cos());
            side.vertices.push(Vertex::new(
                Vec3::new(x, 0.0, 0.0) + n * radius,
                n,
                Vec2::new(s as f32 / segments as f32 * around, x + length * 0.5),
            ));
        }
    }
    let row = segments + 1;
    for s in 0..segments {
        let i = s;
        side.indices
            .extend_from_slice(&[i, i + 1, i + row, i + 1, i + row + 1, i + row]);
    }
    let mut caps = Mesh::default();
    for sign in [-1.0f32, 1.0] {
        let base = caps.vertices.len() as u32;
        let n = Vec3::X * sign;
        let x = sign * length * 0.5;
        caps.vertices
            .push(Vertex::new(Vec3::new(x, 0.0, 0.0), n, Vec2::ZERO));
        for s in 0..=segments {
            let a = s as f32 / segments as f32 * TAU;
            let (y, z) = (a.sin() * radius, a.cos() * radius);
            caps.vertices
                .push(Vertex::new(Vec3::new(x, y, z), n, Vec2::new(z * sign, -y)));
        }
        for s in 0..segments {
            let (a, b) = (base + 1 + s, base + 2 + s);
            if sign > 0.0 {
                caps.indices.extend_from_slice(&[base, b, a]);
            } else {
                caps.indices.extend_from_slice(&[base, a, b]);
            }
        }
    }
    (side, caps)
}

/// A coil of rope (torus in the XZ plane): u goes once around the rope per texture tile, v is
/// meters along it (rounded to whole tiles around the coil).
fn rope_coil(radius: f32, tube: f32, tile: [f32; 2]) -> Mesh {
    let (segments, sides) = (160u32, 24u32);
    let along = whole_tiles(TAU * radius, tile[1]);
    let mut m = Mesh::default();
    for i in 0..=segments {
        let a = i as f32 / segments as f32 * TAU;
        let center = Vec3::new(a.cos(), 0.0, -a.sin()) * radius;
        let out = center.normalize();
        for j in 0..=sides {
            let b = j as f32 / sides as f32 * TAU;
            let n = out * b.cos() + Vec3::Y * b.sin();
            m.vertices.push(Vertex::new(
                center + n * tube,
                n,
                Vec2::new(
                    j as f32 / sides as f32 * tile[0],
                    i as f32 / segments as f32 * along,
                ),
            ));
        }
    }
    let row = sides + 1;
    for i in 0..segments {
        for j in 0..sides {
            let a = i * row + j;
            m.indices
                .extend_from_slice(&[a, a + row, a + 1, a + 1, a + row, a + row + 1]);
        }
    }
    m
}

struct Views {
    list: Vec<(String, DVec3, DVec3, f32)>,
}

impl Views {
    fn new() -> Self {
        let mut list = Vec::new();
        let rows = Surface::ALL.len().div_ceil(COLS);
        let depth = (rows - 1) as f64 * SPACING_Z;
        list.push((
            "overview".to_string(),
            DVec3::new(0.0, 5.2, 7.0),
            DVec3::new(0.0, 0.0, -depth * 0.5),
            46.0,
        ));
        for r in 0..rows {
            let z = -(r as f64) * SPACING_Z;
            for (half, cols) in [("a", 0..4usize), ("b", 4..COLS)] {
                let mid = cols.clone().map(|c| slot(c).x).sum::<f64>() / cols.len() as f64;
                list.push((
                    format!("row{r}{half}"),
                    DVec3::new(mid, 1.25, z + 3.3),
                    DVec3::new(mid, 0.3, z),
                    48.0,
                ));
            }
        }
        for (i, s) in Surface::ALL.iter().enumerate() {
            let at = slot(i);
            list.push((
                format!("{s:?}"),
                at + DVec3::new(0.15, 0.95, 1.45),
                at + DVec3::new(0.0, 0.3, 0.0),
                40.0,
            ));
        }
        Self { list }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let out = args
        .get(1)
        .cloned()
        .unwrap_or_else(|| "screenshots/materials".into());
    let only: Vec<String> = args
        .get(2)
        .map(|s| s.split(',').map(str::to_string).collect())
        .unwrap_or_else(|| {
            let mut v: Vec<String> = vec!["overview".into()];
            for r in 0..4 {
                v.push(format!("row{r}a"));
                v.push(format!("row{r}b"));
            }
            for s in [
                "Planks",
                "PaintedWood",
                "Rock",
                "Gelcoat",
                "Concrete",
                "Bark",
                "EndGrain",
            ] {
                v.push(s.into());
            }
            v
        });
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

    let started = std::time::Instant::now();
    let mats = DemoMaterials::build(&mut app.world.resource_mut::<Assets<Image>>());
    eprintln!(
        "DemoMaterials::build: {:.2} s",
        started.elapsed().as_secs_f32()
    );

    let mut legend = String::new();
    let w = &mut app.world;
    // Ground: the terrain detail texture tinted like a dry meadow.
    let mut ground = mats.get(Surface::Ground);
    ground.base_color = [0.16, 0.17, 0.09, 1.0];
    w.spawn((
        Name::new("Ground"),
        Transform::from_xyz(0.0, 0.0, -3.0),
        MeshRenderer::new(
            Shape::Plane {
                size: [80.0, 80.0],
                subdivisions: 1,
            },
            ground,
        ),
    ));
    let cube = w
        .resource_mut::<Assets<Mesh>>()
        .add(rounded_cube(CUBE, 0.045));
    for (i, &s) in Surface::ALL.iter().enumerate() {
        let at = slot(i);
        let tile = tile_size(s);
        let mat = mats.get(s);
        let _ = writeln!(
            legend,
            "{s:?}: row {} col {} at ({:.1}, {:.1}); tile {:.3} x {:.3} m",
            i / COLS,
            i % COLS,
            at.x,
            at.z,
            tile[0],
            tile[1]
        );
        let sphere_mesh = w.resource_mut::<Assets<Mesh>>().add(sphere(RADIUS, tile));
        w.spawn((
            Name::new(format!("{s:?} sphere")),
            Transform::from_translation(at + DVec3::new(-0.42, RADIUS as f64, 0.0)),
            MeshRenderer::new(sphere_mesh, mat),
        ));
        match s {
            Surface::EndGrain => {
                let (side, caps) = log(0.17, 0.55, tile_size(Surface::Bark));
                let mut meshes = w.resource_mut::<Assets<Mesh>>();
                let (side, caps) = (meshes.add(side), meshes.add(caps));
                let t = Transform::from_translation(at + DVec3::new(0.42, 0.17, 0.0))
                    .with_rotation(Quat::from_rotation_y(-0.5));
                w.spawn((
                    Name::new("EndGrain log bark"),
                    t,
                    MeshRenderer::new(side, mats.get(Surface::Bark)),
                ));
                w.spawn((
                    Name::new("EndGrain log caps"),
                    t,
                    MeshRenderer::new(caps, mat),
                ));
            }
            Surface::Rope => {
                let coil = w
                    .resource_mut::<Assets<Mesh>>()
                    .add(rope_coil(0.22, 0.04, tile));
                for k in 0..3 {
                    w.spawn((
                        Name::new("Rope coil"),
                        Transform::from_translation(
                            at + DVec3::new(0.42, 0.04 + 0.075 * k as f64, 0.0),
                        )
                        .with_rotation(Quat::from_rotation_y(k as f32 * 0.9)),
                        MeshRenderer::new(coil, mat),
                    ));
                }
            }
            _ => {
                w.spawn((
                    Name::new(format!("{s:?} cube")),
                    Transform::from_translation(at + DVec3::new(0.42, CUBE as f64 * 0.5, 0.0))
                        .with_rotation(Quat::from_rotation_y(0.35)),
                    MeshRenderer::new(cube, mat),
                ));
            }
        }
    }
    std::fs::write(format!("{out}/legend.txt"), &legend).expect("legend");
    eprint!("{legend}");

    // A low, warm late-afternoon sun from the front left.
    let to_sun = Vec3::new(-0.62, 0.4, 0.68).normalize();
    w.spawn((
        Name::new("Sun"),
        DirectionalLight {
            illuminance: 70_000.0,
            color: [1.0, 0.86, 0.7],
            ..Default::default()
        },
        Transform::default().looking_at(-to_sun.as_dvec3(), Vec3::Y),
    ));
    let camera = w
        .spawn((
            Name::new("Camera"),
            Camera {
                exposure: Exposure::Manual { ev100: 14.2 },
                ..Camera::perspective(45.0)
            },
            Transform::from_xyz(0.0, 5.0, 7.0),
        ))
        .id();

    // Updates don't draw (a software GPU takes seconds per frame): each picture is one
    // `capture_now` after an update that moves the camera.
    app.world
        .resource_mut::<Schedules>()
        .remove(Render)
        .expect("render schedule");
    app.world
        .resource_mut::<Schedules>()
        .insert(Schedule::new(Render));
    app.update_n(2);
    for (key, eye, target, fov) in Views::new().list {
        if !only.contains(&key) {
            continue;
        }
        *app.world.get_mut::<Transform>(camera).expect("camera") =
            Transform::from_translation(eye).looking_at(target, Vec3::Y);
        app.world
            .get_mut::<Camera>(camera)
            .expect("camera")
            .projection = Projection::Perspective {
            fov_y_deg: fov,
            near: 0.05,
        };
        let started = std::time::Instant::now();
        app.update();
        let path = format!("{out}/{key}.png");
        match capture_now(&mut app.world, &path) {
            Ok(_) => eprintln!("{path} ({:.1} s)", started.elapsed().as_secs_f32()),
            Err(e) => eprintln!("{path}: {e}"),
        }
        if let Some(d) = app.world.get_resource::<RenderDisabled>() {
            eprintln!("no GPU: {}", d.0);
            return;
        }
    }
}
