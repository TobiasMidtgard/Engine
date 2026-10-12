//! Renders the Spell court's props one by one into `screenshots/props/` and prints their
//! triangle counts.
//!
//! ```sh
//! cargo run -p engine_vfx --release --example props              # every prop
//! cargo run -p engine_vfx --release --example props -- anvil wall
//! ```
//!
//! Each prop gets a three-quarter view and a close-up, in daylight. Needs a GPU or lavapipe.
//! Set `PROPS_COLORS=1` to also print each part's mean vertex color (baked paint).

use engine_core::math::{DVec3, Vec3};
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_render::prelude::*;
use engine_render::{MaterialRef, MeshRef, RenderDisabled, capture_now};
use engine_vfx::demo::art::{self, Prop, props};
use engine_vfx::prelude::*;

const W: u32 = 1280;
const H: u32 = 720;

/// A prop to preview: name, builder, view direction (from the target toward the camera), how
/// far to zoom in for the close-up (0..1 of the full view) and what the close-up looks at (the
/// middle of the prop by default).
struct Entry {
    name: &'static str,
    build: fn() -> Prop,
    from: Vec3,
    close: f32,
    focus: Option<Vec3>,
}

fn entries() -> Vec<Entry> {
    let front = Vec3::new(0.8, 0.55, -1.0);
    let back = Vec3::new(0.8, 0.55, 1.0);
    let e = |name, build: fn() -> Prop, from, close| Entry {
        name,
        build,
        from,
        close,
        focus: None,
    };
    vec![
        Entry {
            focus: Some(Vec3::new(5.0, 0.0, 17.0)),
            ..e(
                "floor",
                || props::court::floor(21.0, &engine_vfx::demo::clearings()),
                Vec3::new(0.3, 0.25, 1.0),
                0.08,
            )
        },
        e("portal", props::portal::gate, back, 0.35),
        e("caster", props::lane::caster_pedestal, front, 0.45),
        e("crystal", props::lane::focus_crystal, front, 0.5),
        e(
            "wall",
            props::lane::target_wall,
            Vec3::new(-1.0, 0.45, 0.5),
            0.4,
        ),
        e("obelisk", || props::storm::obelisk(1), front, 0.3),
        e("dais", props::water::dais, front, 0.4),
        e("fountain", props::water::fountain, front, 0.35),
        e("pond", props::water::pond, front, 0.35),
        e("anvil", props::forge::anvil, front, 0.4),
        e("patches", props::beds::footstep_patches, back, 0.25),
        e("plates", props::beds::impact_plates, back, 0.25),
        e("brazier", props::rim::brazier, front, 0.3),
        e("shrubs", || props::rim::shrubs(0), front, 0.45),
        e("golem", golem, back, 0.6),
        e("stone", || props::golem::dropped_stone(0), front, 0.6),
    ]
}

/// The golem assembled standing, for previewing.
fn golem() -> Prop {
    use engine_vfx::demo::art::meshes::moved;
    use props::golem::{HIP, SHOULDER, golem_arm, golem_body, golem_leg};
    let mut p = Prop::new();
    let body = Vec3::Y * 0.9;
    for part in golem_body().parts {
        p.push(part.surface, moved(part.mesh, body, Quat::IDENTITY));
    }
    for s in [-1.0f32, 1.0] {
        for part in golem_arm().parts {
            let at = body + SHOULDER * Vec3::new(s, 1.0, 1.0);
            p.push(
                part.surface,
                moved(part.mesh, at, Quat::from_rotation_z(s * 0.08)),
            );
        }
        for part in golem_leg().parts {
            let at = body + HIP * Vec3::new(s, 1.0, 1.0);
            p.push(part.surface, moved(part.mesh, at, Quat::IDENTITY));
        }
    }
    // Turned to face the light (it faces -Z in the court).
    for part in &mut p.parts {
        let mesh = std::mem::take(&mut part.mesh);
        part.mesh = moved(
            mesh,
            Vec3::ZERO,
            Quat::from_rotation_y(std::f32::consts::PI),
        );
    }
    p
}

fn main() {
    let only: Vec<String> = std::env::args().skip(1).collect();
    let out = std::path::Path::new("screenshots/props");
    std::fs::create_dir_all(out).expect("create screenshots/props");

    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = W;
        w.height = H;
    }
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual { step_secs: 0.05 });
    app.add_plugins((RenderPlugin, VfxPlugin));
    let w = &mut app.world;
    let e = 38f32.to_radians();
    let dir = DVec3::new(
        -0.55 * e.cos() as f64,
        -(e.sin() as f64),
        -0.75 * e.cos() as f64,
    );
    w.spawn((
        DirectionalLight {
            illuminance: 60_000.0,
            ..Default::default()
        },
        Transform::default().looking_at(dir, Vec3::Y),
    ));
    w.spawn((
        Transform::from_xyz(0.0, -0.002, 0.0),
        MeshRenderer::new(
            Shape::Plane {
                size: [200.0, 200.0],
                subdivisions: 1,
            },
            StandardMaterial::hex(0x4f5a3c).with_roughness(0.95),
        ),
    ));
    let camera = w
        .spawn((
            Camera {
                exposure: Exposure::Manual { ev100: 14.0 },
                ..Camera::perspective(45.0)
            },
            Transform::from_xyz(0.0, 2.0, 6.0),
        ))
        .id();
    app.update();
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("no GPU: {}", d.0);
        return;
    }

    let mut total = 0;
    for entry in entries() {
        if !only.is_empty() && !only.iter().any(|o| entry.name.starts_with(o.as_str())) {
            continue;
        }
        let started = std::time::Instant::now();
        let prop = (entry.build)();
        let built = started.elapsed();
        let tris = prop.triangle_count();
        if std::env::var("PROPS_COLORS").is_ok() {
            for part in &prop.parts {
                let n = part.mesh.vertices.len().max(1) as f32;
                let mut sum = [0.0f32; 3];
                let mut lo = 9.0f32;
                for v in &part.mesh.vertices {
                    for (s, c) in sum.iter_mut().zip(v.color) {
                        *s += c / n;
                    }
                    lo = lo.min(v.color[1]);
                }
                println!("  {:?}: mean color {sum:.2?}, min {lo:.2}", part.surface);
            }
        }
        total += tris;
        let mut bounds = prop.parts[0].mesh.aabb();
        for p in &prop.parts {
            bounds = bounds.union(&p.mesh.aabb());
        }
        let parts = art::register(&mut app.world, entry.name, move || prop);
        let spawned: Vec<Entity> = parts
            .into_iter()
            .map(|(mesh, material)| {
                app.world
                    .spawn((
                        Transform::default(),
                        MeshRenderer::new(MeshRef::Handle(mesh), MaterialRef::Handle(material)),
                    ))
                    .id()
            })
            .collect();
        println!(
            "{:>10}: {tris:>8} triangles, built in {:.0} ms",
            entry.name,
            built.as_secs_f64() * 1000.0
        );
        let center = bounds.center().as_dvec3();
        let radius = bounds.half_extents().length().max(0.3) as f64;
        let dir = entry.from.normalize().as_dvec3();
        for (label, k) in [("full", 1.0f64), ("close", entry.close as f64)] {
            let dist = (radius * 2.3 * k).max(0.9);
            let target = match (label, entry.focus) {
                ("close", Some(f)) => f.as_dvec3(),
                ("close", None) => center - DVec3::Y * (radius * 0.15 * (1.0 - k)).min(center.y),
                _ => center,
            };
            let eye = target + dir * dist;
            *app.world.get_mut::<Transform>(camera).unwrap() =
                Transform::from_translation(eye).looking_at(target, Vec3::Y);
            for _ in 0..3 {
                app.update();
            }
            let path = out.join(format!("{}_{label}.png", entry.name));
            match capture_now(&mut app.world, &path) {
                Ok(_) => println!("            {}", path.display()),
                Err(e) => eprintln!("{}: {e}", entry.name),
            }
        }
        for e in spawned {
            app.world.despawn(e);
        }
    }
    println!("     total: {total:>8} triangles (one of each)");
}
