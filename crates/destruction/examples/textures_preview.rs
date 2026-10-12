//! Previews the procedural PBR texture sets (`engine_destruction::textures`): every kind on a
//! wall panel, a cube and a sphere (box-projected meter UVs, like the destruction meshes)
//! under a low sun, rendered headless as close-ups (~1.5 m) and medium shots (~6 m), plus a
//! look across the ground for the ground kinds. Also saves each kind's flat albedo, normal
//! and ORM images and prints generation times.
//!
//! ```sh
//! cargo run -p engine_destruction --release --example textures_preview -- \
//!     [out_dir] [kinds: all | brick,stone,...] [size: default | pixels]
//! # Generation times only (best of 3), no rendering:
//! cargo run -p engine_destruction --release --example textures_preview -- --bench [kinds]
//! ```
//!
//! Needs a GPU or lavapipe (`mesa-vulkan-drivers`) for the renders; the flat images are
//! written either way.

use std::path::{Path, PathBuf};
use std::time::Instant;

use engine_assets::{Assets, Handle};
use engine_core::math::{DVec3, Vec3};
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_destruction::meshing::MeshBuilder;
use engine_destruction::textures::{self, SurfaceKind, TextureSet};
use engine_render::prelude::*;
use engine_render::{RenderDisabled, capture_now};

/// Meters between the booths that show one kind each.
const SPACING: f64 = 40.0;

/// Wall panel size `[width, height]` for a kind, meters.
fn panel_size(kind: SurfaceKind) -> [f32; 2] {
    match kind {
        SurfaceKind::Brick | SurfaceKind::Stone | SurfaceKind::Concrete => [4.8, 3.6],
        SurfaceKind::Plaster | SurfaceKind::Rock => [3.6, 3.0],
        _ => [2.0, 2.0],
    }
}

/// A box from `min` to `max` (mesh-local), every face flat with box-projected meter UVs.
fn slab(b: &mut MeshBuilder, min: Vec3, max: Vec3, color: [f32; 3]) {
    let c = |x: f32, y: f32, z: f32| Vec3::new(x, y, z);
    let (a, z) = (min, max);
    let faces = [
        (
            [
                c(a.x, a.y, z.z),
                c(z.x, a.y, z.z),
                c(z.x, z.y, z.z),
                c(a.x, z.y, z.z),
            ],
            Vec3::Z,
        ),
        (
            [
                c(z.x, a.y, a.z),
                c(a.x, a.y, a.z),
                c(a.x, z.y, a.z),
                c(z.x, z.y, a.z),
            ],
            -Vec3::Z,
        ),
        (
            [
                c(a.x, z.y, z.z),
                c(z.x, z.y, z.z),
                c(z.x, z.y, a.z),
                c(a.x, z.y, a.z),
            ],
            Vec3::Y,
        ),
        (
            [
                c(a.x, a.y, a.z),
                c(z.x, a.y, a.z),
                c(z.x, a.y, z.z),
                c(a.x, a.y, z.z),
            ],
            -Vec3::Y,
        ),
        (
            [
                c(z.x, a.y, z.z),
                c(z.x, a.y, a.z),
                c(z.x, z.y, a.z),
                c(z.x, z.y, z.z),
            ],
            Vec3::X,
        ),
        (
            [
                c(a.x, a.y, a.z),
                c(a.x, a.y, z.z),
                c(a.x, z.y, z.z),
                c(a.x, z.y, a.z),
            ],
            -Vec3::X,
        ),
    ];
    for (p, n) in faces {
        b.quad_facing(p, n, color);
    }
}

/// A smooth sphere with box-projected UVs (how voxel rock gets its texture).
fn sphere(radius: f32, rings: u32, color: [f32; 3]) -> MeshBuilder {
    let mut b = MeshBuilder::new();
    let segs = rings * 2;
    for i in 0..=rings {
        let theta = std::f32::consts::PI * i as f32 / rings as f32;
        for j in 0..=segs {
            let phi = std::f32::consts::TAU * j as f32 / segs as f32;
            let n = Vec3::new(
                theta.sin() * phi.cos(),
                theta.cos(),
                theta.sin() * phi.sin(),
            );
            b.vertex(n * radius, n, color);
        }
    }
    let row = segs + 1;
    for i in 0..rings {
        for j in 0..segs {
            let a = i * row + j;
            let (b1, c1, d1) = (a + 1, a + row, a + row + 1);
            b.triangle(a, b1, c1);
            b.triangle(b1, d1, c1);
        }
    }
    b
}

fn save_flat(dir: &Path, set: &TextureSet) {
    for (name, img) in [
        ("albedo", &set.albedo),
        ("normal", &set.normal),
        ("orm", &set.orm),
    ] {
        let path = dir.join(format!("{}_{name}.png", set.kind.name()));
        match image::RgbaImage::from_raw(img.width, img.height, img.data.clone()) {
            Some(buf) => {
                if let Err(e) = buf.save(&path) {
                    eprintln!("{}: {e}", path.display());
                }
            }
            None => eprintln!("{}: bad image size", path.display()),
        }
    }
}

/// CPU time used by this process so far (all threads), seconds; Linux only.
fn cpu_seconds() -> Option<f64> {
    let stat = std::fs::read_to_string("/proc/self/stat").ok()?;
    // Fields after the parenthesized command name; utime and stime are the 12th and 13th.
    let rest = &stat[stat.rfind(')')? + 2..];
    let f: Vec<&str> = rest.split_whitespace().collect();
    let ticks: f64 = f.get(11)?.parse::<f64>().ok()? + f.get(12)?.parse::<f64>().ok()?;
    Some(ticks / 100.0)
}

/// `all` (or nothing) for every kind, else a comma-separated list of kind names.
fn parse_kinds(arg: Option<&str>) -> Vec<SurfaceKind> {
    match arg {
        None | Some("all") => SurfaceKind::ALL.to_vec(),
        Some(list) => list
            .split(',')
            .map(|n| SurfaceKind::from_name(n.trim()).unwrap_or_else(|| panic!("unknown kind {n}")))
            .collect(),
    }
}

/// Prints each kind's best-of-3 generation time at 512 px wide and at its default size,
/// with the CPU time per set (a busy machine stretches wall time, not CPU time).
fn bench(kinds: &[SurfaceKind]) {
    let run = |kind: SurfaceKind, size: u32| {
        let cpu0 = cpu_seconds();
        let best = (0..3)
            .map(|_| {
                let t0 = Instant::now();
                std::hint::black_box(textures::generate(kind, size, 7));
                t0.elapsed().as_secs_f64() * 1000.0
            })
            .fold(f64::MAX, f64::min);
        let cpu = cpu0
            .zip(cpu_seconds())
            .map_or(f64::NAN, |(a, b)| (b - a) * 1000.0 / 3.0);
        (best, cpu)
    };
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
    println!("{threads} threads");
    for &kind in kinds {
        let (w512, c512) = run(kind, 512);
        let (wd, cd) = run(kind, kind.default_size());
        println!(
            "{:>9}: 512 wide {w512:6.1} ms (cpu {c512:6.0} ms), {} wide {wd:7.1} ms (cpu {cd:6.0} ms)",
            kind.name(),
            kind.default_size(),
        );
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--bench") {
        bench(&parse_kinds(args.get(2).map(String::as_str)));
        return;
    }
    let out = PathBuf::from(
        args.get(1)
            .cloned()
            .unwrap_or_else(|| "texture_shots".into()),
    );
    let kinds = parse_kinds(args.get(2).map(String::as_str));
    let size_override: Option<u32> = args.get(3).and_then(|s| s.parse().ok());
    let flat = out.join("flat");
    std::fs::create_dir_all(&flat).expect("create output dir");

    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut win = app.world.resource_mut::<Window>();
        win.width = 1280;
        win.height = 720;
    }
    let step = 1.0 / 30.0;
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual { step_secs: step });
    app.add_plugins(RenderPlugin);

    // Generate every set (timed), save the flat images, make materials.
    let mut sets = Vec::new();
    for &kind in &kinds {
        let size = size_override.unwrap_or(kind.default_size());
        let t0 = Instant::now();
        let set = textures::generate(kind, size, 7);
        let ms = t0.elapsed().as_secs_f64() * 1000.0;
        let t1 = Instant::now();
        let _ = textures::generate(kind, 512, 7);
        let ms512 = t1.elapsed().as_secs_f64() * 1000.0;
        println!(
            "{:>9}: {}x{} in {ms:7.1} ms (512 wide: {ms512:6.1} ms), tile {:?} m",
            kind.name(),
            set.albedo.width,
            set.albedo.height,
            set.tile_m
        );
        save_flat(&flat, &set);
        sets.push(set);
    }
    // Ground kinds lie on their own ground (and get a shot of it); the rest stand on dirt.
    let is_ground = |k: SurfaceKind| {
        matches!(
            k,
            SurfaceKind::Dirt | SurfaceKind::Gravel | SurfaceKind::Grass
        )
    };
    let ground_kind = |k: SurfaceKind| if is_ground(k) { k } else { SurfaceKind::Dirt };
    let mut ground_materials: Vec<(SurfaceKind, Handle<StandardMaterial>)> = Vec::new();

    let mut pending: Vec<(MeshBuilder, Handle<StandardMaterial>, DVec3)> = Vec::new();
    let mut booths = Vec::new();
    for (i, set) in sets.into_iter().enumerate() {
        let kind = set.kind;
        let bx = i as f64 * SPACING;
        let world = &mut app.world;
        let material = {
            let mut images = world.resource_mut::<Assets<Image>>();
            set.into_material(&mut images)
        };
        let mat = world
            .resource_mut::<Assets<StandardMaterial>>()
            .add(material);
        // Ground for the booth.
        let gk = ground_kind(kind);
        let ground_mat = match ground_materials.iter().find(|(k, _)| *k == gk) {
            Some((_, h)) => *h,
            None => {
                let gs = textures::generate(gk, size_override.unwrap_or(gk.default_size()), 11);
                let m = {
                    let mut images = world.resource_mut::<Assets<Image>>();
                    gs.into_material(&mut images)
                };
                let h = world.resource_mut::<Assets<StandardMaterial>>().add(m);
                ground_materials.push((gk, h));
                h
            }
        };
        let mut g = MeshBuilder::new();
        g.quad_facing(
            [
                Vec3::new(-8.0, 0.0, -6.0),
                Vec3::new(8.0, 0.0, -6.0),
                Vec3::new(8.0, 0.0, 10.0),
                Vec3::new(-8.0, 0.0, 10.0),
            ],
            Vec3::Y,
            [1.0; 3],
        );
        pending.push((g, ground_mat, DVec3::new(bx, 0.0, 0.0)));

        // Wall panel standing on the ground, front face at z = 0, left edge at local x = 0
        // so masonry joints start at the panel's corner.
        let [pw, ph] = panel_size(kind);
        let mut wall = MeshBuilder::new();
        slab(
            &mut wall,
            Vec3::new(0.0, 0.0, -0.3),
            Vec3::new(pw, ph, 0.0),
            [1.0; 3],
        );
        pending.push((wall, mat, DVec3::new(bx - pw as f64 * 0.5, 0.0, 0.0)));
        // A cube (tinted for plaster, as painted walls are) and a sphere in front.
        let tint = if kind == SurfaceKind::Plaster {
            [0.62, 0.72, 0.66]
        } else {
            [1.0; 3]
        };
        let mut cube = MeshBuilder::new();
        slab(&mut cube, Vec3::splat(-0.45), Vec3::splat(0.45), tint);
        pending.push((cube, mat, DVec3::new(bx + 1.25, 0.45, 1.3)));
        pending.push((
            sphere(0.6, 48, [1.0; 3]),
            mat,
            DVec3::new(bx - 1.2, 0.6, 1.5),
        ));
        booths.push((kind, bx, ph));
    }
    for (builder, mat, at) in pending {
        let mesh = app
            .world
            .resource_mut::<Assets<Mesh>>()
            .add(builder.build());
        app.world.spawn((
            Transform::from_translation(at),
            MeshRenderer::new(mesh, mat),
        ));
    }
    app.world.spawn((
        Camera {
            exposure: Exposure::Manual { ev100: 14.6 },
            ..Camera::perspective(50.0)
        },
        Transform::from_xyz(0.0, 1.5, 6.0).looking_at(DVec3::new(0.0, 1.2, 0.0), Vec3::Y),
    ));
    // Low afternoon sun from the front left, raking across the walls.
    app.world.spawn((
        DirectionalLight::default(),
        Transform::default().looking_at(DVec3::new(0.75, -0.42, -0.5), Vec3::Y),
    ));

    app.world.resource_mut::<Time>().pause();
    for (kind, bx, ph) in booths {
        let mid = (ph as f64 * 0.45).min(1.3);
        let mut shots = vec![
            ("close", [bx + 0.35, mid + 0.15, 1.5], [bx - 0.1, mid, 0.0]),
            ("medium", [bx + 1.6, 1.9, 6.5], [bx, 1.1, 0.0]),
        ];
        if is_ground(kind) {
            // Standing on it, looking down and ahead.
            shots.push(("ground", [bx - 3.0, 1.7, 5.5], [bx - 3.4, 0.0, 2.6]));
        }
        for (name, eye, target) in shots {
            {
                let mut q = app.world.query_filtered::<&mut Transform, With<Camera>>();
                for mut c in q.iter_mut(&mut app.world) {
                    *c = Transform::from_translation(DVec3::from_array(eye))
                        .looking_at(DVec3::from_array(target), Vec3::Y);
                }
            }
            app.update_n(3);
            if let Some(d) = app.world.get_resource::<RenderDisabled>() {
                eprintln!("no GPU: {}", d.0);
                return;
            }
            let path = out.join(format!("{}_{name}.png", kind.name()));
            match capture_now(&mut app.world, &path) {
                Ok(_) => println!("{}", path.display()),
                Err(e) => eprintln!("{}: {e}", path.display()),
            }
        }
    }
}
