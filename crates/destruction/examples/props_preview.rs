//! Renders every procedural prop from `engine_destruction::props` headless: an overview of the
//! whole yard and close-ups about 2 m away under a low sun, so bevels, gaps and baked
//! occlusion show.
//!
//! ```sh
//! cargo run -p engine_destruction --release --example props_preview -- [out_dir] [shot filter]
//! ```
//!
//! Needs a GPU or lavapipe (`mesa-vulkan-drivers`). Plain colors stand in for the material
//! textures.

use std::path::PathBuf;

use engine_assets::Assets;
use engine_core::math::{DVec3, Quat, Vec3};
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_destruction::material::DestructibleMaterial;
use engine_destruction::props::{self, PropPart, prop_bounds, prop_triangles};
use engine_render::prelude::*;
use engine_render::{RenderDisabled, capture_now};

/// A prop in the yard: parts, ground position (x, z), yaw (radians), lift above the ground.
struct Placed {
    name: &'static str,
    parts: Vec<PropPart>,
    at: [f32; 2],
    yaw: f32,
    lift: f32,
}

fn yard() -> Vec<Placed> {
    let p = |name, parts, at: [f32; 2], yaw: f32| Placed {
        name,
        parts,
        at,
        yaw,
        lift: 0.0,
    };
    let mut v = vec![
        p("pallet", props::pallet(1), [-6.5, 0.0], 0.1),
        p(
            "wooden_crate",
            props::wooden_crate(Vec3::new(1.0, 0.8, 0.8), 2),
            [-4.8, 0.2],
            -0.25,
        ),
        p(
            "military_crate",
            props::military_crate(Vec3::new(1.0, 0.45, 0.5), 3),
            [-3.2, -0.2],
            0.35,
        ),
        p("steel_drum", props::steel_drum(2, 4), [-1.9, 0.1], 0.0),
        p("steel_drum_b", props::steel_drum(0, 5), [-1.3, 0.55], 1.2),
        p("sandbag", props::sandbag(6), [-0.4, -0.6], 0.3),
        p(
            "sandbag_wall",
            props::sandbag_wall(2.5, 4, 7),
            [0.6, 0.8],
            0.0,
        ),
        p(
            "jersey_barrier",
            props::jersey_barrier(8),
            [3.6, 0.2],
            -0.15,
        ),
        p(
            "i_beam",
            props::steel_i_beam(4.0, 0.3, true),
            [1.8, -2.2],
            0.05,
        ),
        p(
            "timber",
            props::timber(Vec3::new(3.0, 0.2, 0.2), 9),
            [1.8, -1.4],
            -0.04,
        ),
        p(
            "concrete_pillar",
            props::concrete_pillar(Vec3::new(0.6, 3.0, 0.6), 10),
            [6.0, -1.0],
            0.0,
        ),
        p(
            "stone_column",
            props::stone_column(4.0, 0.3, 11),
            [7.8, -1.0],
            0.0,
        ),
        p("scaffold", props::scaffold_tower(12), [10.2, 0.3], 0.0),
        p("fence", props::chain_link_fence(6.0, 13), [2.0, -6.0], 0.0),
    ];
    // A crate on the pallet, and a gable roof on four timber posts.
    let mut crate_on_pallet = p(
        "crate_on_pallet",
        props::wooden_crate(Vec3::splat(0.75), 14),
        [-6.5, 0.0],
        0.15,
    );
    crate_on_pallet.lift = 0.144;
    v.push(crate_on_pallet);
    for (i, (x, z)) in [(-1.8f32, -1.3f32), (1.8, -1.3), (-1.8, 1.3), (1.8, 1.3)]
        .into_iter()
        .enumerate()
    {
        let mut post = p(
            "post",
            props::timber(Vec3::new(0.15, 2.3, 0.15), 20 + i as u64),
            [-3.5 + x, -5.0 + z],
            0.0,
        );
        post.yaw = 0.0;
        v.push(post);
    }
    let mut roof = p(
        "roof",
        props::roof_tiles(4.4, 3.6, 35f32.to_radians(), 15),
        [-3.5, -5.0],
        0.0,
    );
    roof.lift = 2.3 - 0.12;
    v.push(roof);
    v
}

/// (file name, camera position, look-at point)
fn shots() -> Vec<(&'static str, [f64; 3], [f64; 3])> {
    vec![
        ("00_overview", [1.5, 6.5, 12.5], [1.5, 0.6, -1.5]),
        ("01_crates", [-5.4, 1.5, 2.6], [-5.2, 0.45, 0.0]),
        ("02_military_crate", [-2.7, 1.1, 1.3], [-3.2, 0.25, -0.2]),
        ("03_drums", [-1.0, 1.4, 2.2], [-1.6, 0.45, 0.3]),
        ("04_sandbags", [0.2, 1.3, 3.2], [0.3, 0.3, 0.5]),
        ("05_barrier", [4.6, 1.2, 2.4], [3.7, 0.4, 0.2]),
        ("06_beam_timber", [3.4, 1.0, -0.2], [2.3, 0.15, -1.8]),
        ("07_pillar_column", [6.9, 1.8, 2.2], [6.9, 1.5, -1.0]),
        ("08_column_capital", [7.2, 3.6, 0.3], [7.8, 3.5, -1.0]),
        ("09_scaffold", [11.4, 1.8, 3.4], [10.2, 1.1, 0.3]),
        ("10_roof", [-1.0, 2.4, -1.5], [-3.5, 2.2, -5.0]),
        ("11_fence", [3.2, 1.3, -4.4], [2.0, 1.0, -6.0]),
        ("12_crate_close", [-4.25, 1.05, 1.15], [-4.85, 0.45, 0.2]),
        ("13_column_base", [7.3, 0.9, 0.2], [7.8, 0.35, -1.0]),
        ("14_barrier_end", [5.4, 0.9, 1.0], [5.0, 0.45, 0.0]),
        ("15_beam_end", [0.4, 0.7, -1.4], [-0.1, 0.25, -2.2]),
        ("16_scaffold_close", [9.8, 1.4, 1.6], [9.4, 1.0, 0.85]),
    ]
}

fn surface(m: DestructibleMaterial) -> StandardMaterial {
    match m {
        DestructibleMaterial::Wood => StandardMaterial::hex(0x9c7c58).with_roughness(0.78),
        DestructibleMaterial::Metal => StandardMaterial::hex(0x8e9196)
            .with_roughness(0.42)
            .with_metallic(0.85),
        DestructibleMaterial::Concrete => StandardMaterial::hex(0xa9a59c).with_roughness(0.92),
        DestructibleMaterial::Stone => StandardMaterial::hex(0xcbc0a8).with_roughness(0.85),
        DestructibleMaterial::Dirt => StandardMaterial::hex(0xa08d68).with_roughness(0.95),
        DestructibleMaterial::Brick => StandardMaterial::hex(0xa25a3e).with_roughness(0.9),
        _ => StandardMaterial::hex(0x9a9a9a).with_roughness(0.8),
    }
}

fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    commands.spawn((
        Camera {
            exposure: Exposure::Manual { ev100: 14.3 },
            ..Camera::perspective(50.0)
        },
        Transform::from_xyz(1.5, 6.5, 12.5).looking_at(DVec3::new(1.5, 0.6, -1.5), Vec3::Y),
    ));
    // Low afternoon sun from the left, slightly in front: grazing light on bevels and gaps.
    commands.spawn((
        DirectionalLight::default(),
        Transform::default().looking_at(DVec3::new(0.82, -0.4, -0.42), Vec3::Y),
    ));
    commands.spawn((
        Transform::default(),
        MeshRenderer::new(
            Shape::Plane {
                size: [400.0, 400.0],
                subdivisions: 1,
            },
            StandardMaterial::hex(0x6e695f).with_roughness(0.95),
        ),
    ));
    let mats: Vec<_> = DestructibleMaterial::ALL
        .iter()
        .map(|m| materials.add(surface(*m)))
        .collect();
    let mut total = 0;
    for item in yard() {
        let (lo, hi) = prop_bounds(&item.parts);
        let tris = prop_triangles(&item.parts);
        total += tris;
        println!(
            "{:<16} {:>6} triangles, size {:.3}",
            item.name,
            tris,
            hi - lo
        );
        let t = Transform::from_xyz(
            item.at[0] as f64,
            (item.lift - lo.y) as f64,
            item.at[1] as f64,
        )
        .with_rotation(Quat::from_rotation_y(item.yaw));
        for part in item.parts {
            let mesh = meshes.add(part.mesh.build());
            commands.spawn((t, MeshRenderer::new(mesh, mats[part.material.index()])));
        }
    }
    println!("total {total} triangles");
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let out = PathBuf::from(args.get(1).cloned().unwrap_or_else(|| "props_shots".into()));
    let filter = args.get(2).cloned().unwrap_or_default();
    std::fs::create_dir_all(&out).expect("create output dir");

    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut win = app.world.resource_mut::<Window>();
        win.width = 1280;
        win.height = 720;
    }
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 30.0,
        });
    app.add_plugins(RenderPlugin).add_systems(Startup, setup);
    app.update_n(2);
    for (name, eye, target) in shots() {
        if !name.contains(filter.as_str()) {
            continue;
        }
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
        let path = out.join(format!("{name}.png"));
        match capture_now(&mut app.world, &path) {
            Ok(_) => println!("{}", path.display()),
            Err(e) => eprintln!("{name}: {e}"),
        }
    }
}
