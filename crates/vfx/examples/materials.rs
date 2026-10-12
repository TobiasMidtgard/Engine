//! Renders every generated court material on a sphere, a beveled slab and a floor, by day and
//! at dusk, from a wide and a close view, into `screenshots/vfx/materials/`. Also prints how
//! long generation took and how much texture memory each material uses.
//!
//! ```sh
//! cargo run -p engine_vfx --release --example materials                  # every surface
//! cargo run -p engine_vfx --release --example materials -- flagstone rock # some surfaces
//! cargo run -p engine_vfx --release --example materials -- --time        # timing only
//! ```
//!
//! Filters match surface names (case-insensitive prefix). `--day`, `--dusk`, `--wide` and
//! `--close` limit the shots. Needs a GPU or lavapipe.

use std::time::Instant;

use engine_assets::{Assets, Handle};
use engine_core::math::{DVec3, Vec2, Vec3};
use engine_core::prelude::*;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, Vertex, capture_now};
use engine_vfx::demo::art::Surface;
use engine_vfx::demo::art::materials::{SURFACES, attach, generate};

const W: u32 = 960;
const H: u32 = 540;

#[derive(Component)]
struct Sun;

/// A UV sphere whose UVs are meters along the surface; the seam lands on a whole texture
/// repeat for `uv_scale` so it does not show.
fn sphere(radius: f32, uv_scale: [f32; 2]) -> Mesh {
    let segments = 160u32;
    let rings = 80u32;
    let around = std::f32::consts::TAU * radius;
    let repeats = (around * uv_scale[0]).round().max(1.0);
    let u_len = repeats / uv_scale[0];
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    for r in 0..=rings {
        let v = r as f32 / rings as f32;
        let phi = v * std::f32::consts::PI;
        for s in 0..=segments {
            let u = s as f32 / segments as f32;
            let theta = u * std::f32::consts::TAU;
            let n = Vec3::new(phi.sin() * theta.cos(), phi.cos(), -phi.sin() * theta.sin());
            vertices.push(Vertex::new(
                n * radius,
                n,
                Vec2::new(u * u_len, phi * radius),
            ));
        }
    }
    let row = segments + 1;
    for r in 0..rings {
        for s in 0..segments {
            let i = r * row + s;
            indices.extend_from_slice(&[i, i + row, i + 1, i + 1, i + row, i + row + 1]);
        }
    }
    Mesh::new(vertices, indices)
}

/// A box with rounded edges (`radius`), UVs in meters projected per face.
fn rounded_box(size: Vec3, radius: f32) -> Mesh {
    let h = size * 0.5;
    let r = radius.min(h.min_element() * 0.99);
    // Sample positions along one face axis: dense across the bevels, ~4 cm in the middle.
    let axis = |half: f32| -> Vec<f32> {
        let mut c = Vec::new();
        let bevel = 8;
        for k in 0..bevel {
            let a = (1.0 - k as f32 / bevel as f32) * std::f32::consts::FRAC_PI_4;
            c.push(-(half - r) - r * a.tan());
        }
        let flat = (((half - r) * 2.0 / 0.04).ceil() as usize).max(1);
        for k in 0..=flat {
            c.push(-(half - r) + (half - r) * 2.0 * k as f32 / flat as f32);
        }
        for k in 1..=bevel {
            let a = k as f32 / bevel as f32 * std::f32::consts::FRAC_PI_4;
            c.push((half - r) + r * a.tan());
        }
        c
    };
    let mut vertices: Vec<Vertex> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();
    // (normal axis, u axis, v axis)
    let faces = [
        (Vec3::X, Vec3::NEG_Z, Vec3::NEG_Y),
        (Vec3::NEG_X, Vec3::Z, Vec3::NEG_Y),
        (Vec3::Y, Vec3::X, Vec3::Z),
        (Vec3::NEG_Y, Vec3::X, Vec3::NEG_Z),
        (Vec3::Z, Vec3::X, Vec3::NEG_Y),
        (Vec3::NEG_Z, Vec3::NEG_X, Vec3::NEG_Y),
    ];
    for (n, ua, va) in faces {
        let us = axis(ua.abs().dot(h));
        let vs = axis(va.abs().dot(h));
        let base = vertices.len() as u32;
        for &v in &vs {
            for &u in &us {
                let q = n * n.abs().dot(h) + ua * u + va * v;
                let inner = q.clamp(-(h - Vec3::splat(r)), h - Vec3::splat(r));
                let nn = (q - inner).normalize();
                let p = inner + nn * r;
                let uv = Vec2::new(p.dot(ua), p.dot(va));
                vertices.push(Vertex::new(p, nn, uv));
            }
        }
        let row = us.len() as u32;
        for j in 0..vs.len() as u32 - 1 {
            for i in 0..row - 1 {
                let a = base + j * row + i;
                let quad = [a, a + row, a + 1, a + 1, a + row, a + row + 1];
                for t in quad.chunks(3) {
                    let (p0, p1, p2) = (
                        vertices[t[0] as usize].position,
                        vertices[t[1] as usize].position,
                        vertices[t[2] as usize].position,
                    );
                    let face_n =
                        (Vec3::from(p1) - Vec3::from(p0)).cross(Vec3::from(p2) - Vec3::from(p0));
                    if face_n.dot(n) >= 0.0 {
                        indices.extend_from_slice(t);
                    } else {
                        indices.extend_from_slice(&[t[0], t[2], t[1]]);
                    }
                }
            }
        }
    }
    Mesh::new(vertices, indices)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flags: Vec<&str> = args
        .iter()
        .filter(|a| a.starts_with("--"))
        .map(|a| a.as_str())
        .collect();
    let only: Vec<String> = args
        .iter()
        .filter(|a| !a.starts_with("--"))
        .map(|a| a.to_lowercase())
        .collect();
    let has = |f: &str| flags.contains(&f);
    let surfaces: Vec<Surface> = SURFACES
        .into_iter()
        .filter(|s| {
            let name = format!("{s:?}").to_lowercase();
            only.is_empty() || only.iter().any(|o| name.starts_with(o.as_str()))
        })
        .collect();

    // Generate everything first, timed (this is the cost the court pays at startup).
    let start = Instant::now();
    let mut made = Vec::new();
    let mut total_bytes = 0usize;
    for &s in &surfaces {
        let t = Instant::now();
        let (material, images) = generate(s, 0);
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        let dims = format!("{}x{}", images.albedo.width, images.albedo.height);
        let bytes = images.bytes();
        total_bytes += bytes;
        println!(
            "{:<12} {:>7.1} ms  {:>9}  {:>6.1} MiB  {:.2}x{:.2} m",
            format!("{s:?}"),
            ms,
            dims,
            bytes as f64 / (1024.0 * 1024.0),
            1.0 / material.uv_scale[0],
            1.0 / material.uv_scale[1],
        );
        made.push((s, material, images));
    }
    println!(
        "total: {:.0} ms, {:.1} MiB of textures (with mips)",
        start.elapsed().as_secs_f64() * 1000.0,
        total_bytes as f64 / (1024.0 * 1024.0)
    );
    if has("--time") {
        return;
    }

    let out = std::path::Path::new("screenshots/vfx/materials");
    std::fs::create_dir_all(out).expect("create screenshots/vfx/materials");
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = W;
        w.height = H;
    }
    app.add_plugins(RenderPlugin);
    app.world
        .spawn((Sun, DirectionalLight::default(), Transform::default()));
    let camera = app
        .world
        .spawn((Camera::perspective(50.0), Transform::default()))
        .id();
    app.update();
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("no GPU: {}", d.0);
        return;
    }

    let lights: Vec<(&str, f32, f32, f32)> = [
        ("day", 90_000.0, 35.0f32, 14.5f32),
        ("dusk", 300.0, 3.0, 5.5),
    ]
    .into_iter()
    .filter(|(l, ..)| !(has("--day") || has("--dusk")) || has(&format!("--{l}")))
    .collect();
    // (label, camera position, look at, vertical fov)
    let views: Vec<(&str, DVec3, DVec3, f32)> = [
        (
            "wide",
            DVec3::new(0.0, 1.35, 2.5),
            DVec3::new(0.05, 0.3, 0.0),
            50.0f32,
        ),
        (
            "close",
            DVec3::new(-0.05, 0.75, 0.95),
            DVec3::new(-0.3, 0.32, 0.0),
            40.0,
        ),
    ]
    .into_iter()
    .filter(|(v, ..)| !(has("--wide") || has("--close")) || has(&format!("--{v}")))
    .collect();

    for (surface, material, images) in made {
        let name = format!("{surface:?}").to_lowercase();
        let material = attach(
            material,
            images,
            &mut app.world.resource_mut::<Assets<Image>>(),
        );
        let handle: Handle<StandardMaterial> = app
            .world
            .resource_mut::<Assets<StandardMaterial>>()
            .add(material);
        let mut spawned = Vec::new();
        let mut mesh = |app: &mut App, mesh: Mesh, at: Transform, mat: Handle<StandardMaterial>| {
            let h = app.world.resource_mut::<Assets<Mesh>>().add(mesh);
            spawned.push(
                app.world
                    .spawn((
                        at,
                        MeshRenderer::new(MeshRef::Handle(h), MaterialRef::Handle(mat)),
                    ))
                    .id(),
            );
        };
        let floor = Shape::Plane {
            size: [12.0, 12.0],
            subdivisions: 8,
        }
        .mesh();
        if surface == Surface::Water {
            // Water over a sandy bed so the transparency reads.
            let bed = app
                .world
                .resource_mut::<Assets<StandardMaterial>>()
                .add(StandardMaterial::hex(0x8a7a60).with_roughness(0.9));
            mesh(
                &mut app,
                floor.clone(),
                Transform::from_xyz(0.0, -0.25, 0.0),
                bed,
            );
            mesh(&mut app, floor, Transform::from_xyz(0.0, 0.0, 0.0), handle);
        } else {
            mesh(&mut app, floor, Transform::default(), handle);
        }
        mesh(
            &mut app,
            sphere(0.42, material.uv_scale),
            Transform::from_xyz(-0.55, 0.42, 0.0),
            handle,
        );
        mesh(
            &mut app,
            rounded_box(Vec3::new(0.9, 0.32, 0.8), 0.035),
            Transform::from_xyz(0.6, 0.16, 0.05),
            handle,
        );

        for &(label, lux, elevation, ev) in &lights {
            {
                let mut q = app
                    .world
                    .query_filtered::<(&mut DirectionalLight, &mut Transform), With<Sun>>();
                for (mut light, mut t) in q.iter_mut(&mut app.world) {
                    light.illuminance = lux;
                    let e = elevation.to_radians();
                    // Raking light from the front left, over the camera's shoulder.
                    let dir = DVec3::new(
                        0.75 * e.cos() as f64,
                        -(e.sin() as f64),
                        -0.66 * e.cos() as f64,
                    );
                    *t = Transform::default().looking_at(dir, Vec3::Y);
                }
                let mut cam = app.world.get_mut::<Camera>(camera).unwrap();
                cam.exposure = Exposure::Manual { ev100: ev };
            }
            for &(view, eye, target, fov) in &views {
                {
                    let mut cam = app.world.get_mut::<Camera>(camera).unwrap();
                    *cam = Camera {
                        exposure: cam.exposure,
                        ..Camera::perspective(fov)
                    };
                    let mut t = app.world.get_mut::<Transform>(camera).unwrap();
                    *t = Transform::from_translation(eye).looking_at(target, Vec3::Y);
                }
                for _ in 0..3 {
                    app.update();
                }
                let path = out.join(format!("{label}_{view}_{name}.png"));
                match capture_now(&mut app.world, &path) {
                    Ok(_) => println!("{}", path.display()),
                    Err(e) => eprintln!("{name}: {e}"),
                }
            }
        }
        for e in spawned {
            app.world.despawn(e);
        }
        app.update();
    }
}
