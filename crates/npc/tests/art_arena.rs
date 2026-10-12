//! The arena's procedural art: builds it with the renderer's asset stores, checks the budget
//! (triangles, build time) and that every piece of art sits on its collider, and renders
//! close-ups of the stone, sand, wood, metal and cloth when a GPU or a software Vulkan driver
//! (lavapipe) is available (otherwise those shots are skipped with a note).
//!
//! The PNGs land in the test's temp dir; set `NPC_SCREENSHOT_DIR` to also copy them there.

use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_npc::*;
use engine_render::prelude::*;
use engine_render::{MeshRef, RenderDisabled, capture_now};
use engine_world::WorldPlugin;
use engine_world::physics::{Collider, ColliderShape};

const W: u32 = 960;
const H: u32 = 540;

fn app() -> App {
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = W;
        w.height = H;
    }
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual { step_secs: 0.1 });
    app.add_plugins((WorldPlugin, RenderPlugin, NpcPlugin));
    let w = &mut app.world;
    w.spawn((
        DirectionalLight {
            illuminance: 90_000.0,
            ..Default::default()
        },
        Transform::default().looking_at(DVec3::new(-0.5, -0.7, 0.6), Vec3::Y),
    ));
    w.spawn((
        Transform::default(),
        MeshRenderer::new(
            Shape::Plane {
                size: [220.0, 220.0],
                subdivisions: 1,
            },
            StandardMaterial::hex(0x8c7a5b).with_roughness(0.95),
        ),
    ));
    {
        let mut commands = app.world.commands();
        demo::spawn_demo(&mut commands, DVec3::ZERO);
    }
    app.world.flush();
    app
}

/// Mesh bounds of `e`'s rendered children in `e`'s frame, keeping only vertices `keep` likes.
fn child_bounds(app: &mut App, e: Entity, keep: impl Fn(Vec3) -> bool) -> Option<(Vec3, Vec3)> {
    let children: Vec<Entity> = app
        .world
        .get::<Children>(e)
        .map(|c| c.iter().collect())
        .unwrap_or_default();
    let meshes = app.world.resource::<engine_assets::Assets<Mesh>>();
    let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    let mut any = false;
    for c in children {
        let (Some(t), Some(r)) = (
            app.world.get::<Transform>(c),
            app.world.get::<MeshRenderer>(c),
        ) else {
            continue;
        };
        let MeshRef::Handle(h) = r.mesh.clone() else {
            continue;
        };
        let Some(mesh) = meshes.get(h) else {
            continue;
        };
        let m = Mat4::from_rotation_translation(t.rotation, t.translation.as_vec3());
        for v in &mesh.vertices {
            let p = m.transform_point3(Vec3::from_array(v.position));
            if keep(p) {
                lo = lo.min(p);
                hi = hi.max(p);
                any = true;
            }
        }
    }
    any.then_some((lo, hi))
}

#[test]
fn arena_art_fits_the_budget_and_the_colliders() {
    let mut app = app();
    let d = app.world.resource::<Diagnostics>();
    let tris = d.get("npc.arena_triangles").expect("arena dressed");
    let ms = d.get("npc.arena_build_ms").unwrap_or(0.0);
    eprintln!("arena art: {tris:.0} triangles, built in {ms:.0} ms");
    assert!(tris > 300_000.0, "arena has real detail: {tris}");
    assert!(tris <= 1_500_000.0, "arena triangle budget: {tris}");

    // Every dressed collider: the art covers it and stays within 5 cm of it (below the
    // collider's floor the art may reach into the ground or hold it up).
    let tol = 0.05;
    let mut q = app.world.query::<(Entity, &Collider)>();
    let solids: Vec<(Entity, ColliderShape)> = q
        .iter(&app.world)
        .map(|(e, c)| (e, c.shape.clone()))
        .collect();
    let mut checked = 0;
    for (e, shape) in solids {
        match shape {
            ColliderShape::Cuboid { half_extents: h } => {
                let Some((lo, hi)) = child_bounds(&mut app, e, |p| p.y > -h.y + 0.01) else {
                    continue;
                };
                for (axis, (a, b)) in [(0, (lo.x, hi.x)), (2, (lo.z, hi.z))] {
                    let half = h[axis];
                    assert!(
                        a >= -half - tol && b <= half + tol,
                        "{e:?}: art {a:.3}..{b:.3} outside collider ±{half:.3} on axis {axis}"
                    );
                    assert!(
                        a <= -half + tol && b >= half - tol,
                        "{e:?}: art {a:.3}..{b:.3} leaves collider ±{half:.3} bare on axis {axis}"
                    );
                }
                assert!(
                    (hi.y - h.y).abs() <= tol,
                    "{e:?}: art top {:.3} vs collider top {:.3}",
                    hi.y,
                    h.y
                );
                checked += 1;
            }
            ColliderShape::Cylinder {
                radius,
                half_height,
            } => {
                // Where fighters can touch it: the lower two meters.
                let reach = |p: Vec3| p.y < -half_height + 2.0;
                let Some((lo, hi)) = child_bounds(&mut app, e, reach) else {
                    continue;
                };
                let r = lo.x.abs().max(hi.x).max(lo.z.abs()).max(hi.z);
                assert!(
                    r <= radius + tol && r >= radius - 0.1,
                    "{e:?}: art radius {r:.3} vs collider {radius:.3}"
                );
                let Some((_, top)) = child_bounds(&mut app, e, |_| true) else {
                    continue;
                };
                assert!(
                    (top.y - half_height).abs() <= tol,
                    "{e:?}: art top {:.3} vs collider top {half_height:.3}",
                    top.y
                );
                checked += 1;
            }
            _ => {}
        }
    }
    // Walls, crates, platform, steps, ramp, wall-run wall, pillars and barrels.
    assert!(
        checked >= 7 + 4 + 1 + 3 + 1 + 1 + 8 + 4,
        "checked {checked} pieces"
    );
}

fn camera(app: &mut App, eye: DVec3, target: DVec3, fov: f32) -> Entity {
    app.world
        .spawn((
            Camera {
                active: false,
                ..Camera::perspective(fov)
            },
            Transform::from_translation(eye).looking_at(target, Vec3::Y),
        ))
        .id()
}

fn shoot(app: &mut App, cam: Entity, name: &str) {
    let mut q = app.world.query::<(Entity, &mut Camera)>();
    for (e, mut c) in q.iter_mut(&mut app.world) {
        c.active = e == cam;
    }
    app.update_n(2);
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    capture_now(&mut app.world, &path).expect("screenshot");
    if let Ok(dir) = std::env::var("NPC_SCREENSHOT_DIR") {
        let _ = std::fs::create_dir_all(&dir);
        std::fs::copy(&path, std::path::Path::new(&dir).join(name)).expect("copy screenshot");
    }
    eprintln!("wrote {}", path.display());
}

#[test]
fn arena_art_close_ups() {
    let mut app = app();
    let v = |x: f64, y: f64, z: f64| DVec3::new(x, y, z);
    let ring = demo::DUEL_RING;
    let lane = demo::SPARRING_LANE;
    let shots = [
        (
            "art_stands.png",
            v(-5.0, 1.8, -36.0),
            v(-9.5, 1.6, -41.5),
            60.0,
        ),
        ("art_gate.png", v(1.5, 2.2, -33.5), v(0.0, 4.0, -47.5), 60.0),
        (
            "art_stands_far.png",
            v(10.0, 9.0, 2.0),
            v(-14.0, 3.0, 40.0),
            60.0,
        ),
        (
            "art_duel_ring.png",
            ring + v(-2.0, 1.4, -3.4),
            ring + v(0.5, 0.0, 0.5),
            60.0,
        ),
        (
            "art_barrel.png",
            v(-3.0, 1.3, -12.6),
            v(-1.4, 0.6, -10.2),
            55.0,
        ),
        ("art_rubble.png", v(7.7, 1.5, -2.7), v(8.6, 0.1, -5.7), 55.0),
        (
            "art_crates.png",
            v(-7.0, 1.7, -4.5),
            v(-4.0, 1.0, -1.5),
            60.0,
        ),
        (
            "art_timber_wall.png",
            v(3.6, 1.5, 3.8),
            v(5.6, 0.5, 6.5),
            60.0,
        ),
        (
            "art_brazier.png",
            v(-3.0, 1.7, -35.8),
            v(-4.7, 1.0, -38.3),
            55.0,
        ),
        (
            "art_banner.png",
            v(-7.5, 6.0, -35.0),
            v(-11.4, 7.2, -45.6),
            55.0,
        ),
        (
            "art_pillar.png",
            v(-3.5, 1.7, -12.5),
            v(-6.9, 2.6, -16.6),
            60.0,
        ),
        (
            "art_sparring.png",
            lane + v(-3.4, 1.6, -2.8),
            lane + v(-1.7, 1.0, 0.4),
            60.0,
        ),
        (
            "art_platform.png",
            v(-8.5, 2.6, -4.5),
            v(-16.0, 1.0, 2.0),
            60.0,
        ),
        ("art_ramp.png", v(17.0, 2.5, 14.5), v(12.0, 1.2, 7.0), 60.0),
        (
            "art_aerial.png",
            v(0.0, 55.0, -85.0),
            v(0.0, 0.0, 0.0),
            50.0,
        ),
    ];
    app.update_n(3);
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return;
    }
    let cams: Vec<(Entity, &str)> = shots
        .iter()
        .map(|&(name, eye, target, fov)| (camera(&mut app, eye, target, fov), name))
        .collect();
    for (cam, name) in cams {
        shoot(&mut app, cam, name);
    }
}
