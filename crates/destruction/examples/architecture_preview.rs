//! Renders the procedural fortress architecture headless: a small castle (four curtain walls
//! around a 70 × 50 m courtyard, corner towers, a gatehouse and a breached section) with plain
//! colored materials and a low sun, from a few establishing and close-up views.
//!
//! ```sh
//! cargo run -p engine_destruction --release --example architecture_preview -- [out_dir] [WxH] [shot] [--textured]
//! ```
//!
//! `--textured` swaps the plain colors for the procedural PBR sets of
//! [`engine_destruction::textures`] (files get a `_tex` suffix), which shows how the modeled
//! joints line up with the painted ones. Needs a GPU or lavapipe (`mesa-vulkan-drivers`).

use std::f32::consts::{FRAC_PI_2, PI};
use std::path::PathBuf;
use std::time::Instant;

use engine_assets::Assets;
use engine_core::math::{DVec3, Quat, Vec3};
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_destruction::DestructibleMaterial;
use engine_destruction::architecture::{self, ArchPart, TowerTop};
use engine_destruction::textures::{self, SurfaceKind};
use engine_render::prelude::*;
use engine_render::{RenderDisabled, capture_now};

/// (file name, camera position, look-at point, vertical field of view in degrees)
fn shots() -> Vec<(&'static str, [f64; 3], [f64; 3], f32)> {
    vec![
        (
            "01_establishing",
            [-50.0, 21.0, -60.0],
            [4.0, 3.0, 4.0],
            50.0,
        ),
        (
            "02_outside_three_quarter",
            [-34.0, 1.7, -52.0],
            [-8.0, 7.0, -27.0],
            60.0,
        ),
        ("03_gate_close", [5.5, 1.7, -42.0], [0.0, 5.5, -28.0], 55.0),
        (
            "04_parapet_close",
            [-24.0, 3.0, -36.0],
            [-19.0, 8.6, -27.5],
            50.0,
        ),
        ("05_courtyard", [14.0, 1.7, -14.0], [-22.0, 6.0, 22.0], 60.0),
        ("06_breach", [3.0, 1.7, 12.0], [0.0, 3.5, 26.0], 55.0),
        (
            "07_breach_outside",
            [-9.0, 7.5, 40.0],
            [0.5, 3.0, 27.0],
            55.0,
        ),
        (
            "08_wall_face",
            [-20.0, 2.5, -40.0],
            [-20.0, 4.5, -28.0],
            60.0,
        ),
        ("09_breach_core", [-1.0, 9.0, 33.0], [-1.0, 4.5, 26.5], 50.0),
    ]
}

/// One placed piece: parts, position of its local origin, yaw (radians, about +Y).
struct Piece {
    parts: Vec<ArchPart>,
    pos: Vec3,
    yaw: f32,
}

#[derive(Resource)]
struct Fortress(Vec<Piece>);

/// Render with the procedural texture sets instead of plain colors.
#[derive(Resource)]
struct Textured(bool);

fn texture_kind(m: DestructibleMaterial) -> SurfaceKind {
    match m {
        DestructibleMaterial::Wood => SurfaceKind::Wood,
        DestructibleMaterial::Metal => SurfaceKind::Metal,
        DestructibleMaterial::Rock => SurfaceKind::Rock,
        DestructibleMaterial::Dirt => SurfaceKind::Dirt,
        DestructibleMaterial::Plaster => SurfaceKind::Plaster,
        _ => SurfaceKind::Stone,
    }
}

fn textured(kind: SurfaceKind, images: &mut Assets<Image>) -> StandardMaterial {
    textures::generate(kind, kind.default_size(), 7).into_material(images)
}

fn color(m: DestructibleMaterial) -> StandardMaterial {
    match m {
        DestructibleMaterial::Stone => StandardMaterial::hex(0xc2b8a3).with_roughness(0.9),
        DestructibleMaterial::Wood => StandardMaterial::hex(0x7a5636).with_roughness(0.85),
        DestructibleMaterial::Metal => StandardMaterial::hex(0x45474b)
            .with_roughness(0.55)
            .with_metallic(0.7),
        DestructibleMaterial::Rock => StandardMaterial::hex(0x5c6068).with_roughness(0.8),
        DestructibleMaterial::Dirt => StandardMaterial::hex(0x7a6650).with_roughness(0.95),
        DestructibleMaterial::Plaster => StandardMaterial::hex(0xf0ece4).with_roughness(0.8),
        _ => StandardMaterial::hex(0xaaaaaa),
    }
}

/// Assemble the castle. The courtyard spans x ∈ [−35, 35], z ∈ [−25, 25]; the gate faces −Z.
fn build() -> Vec<Piece> {
    let t = 3.0;
    let h = 9.0;
    let (cx, cz) = (35.0 + t * 0.5, 25.0 + t * 0.5);
    let r = 5.0;
    let mut v = Vec::new();
    let mut put = |parts: Vec<ArchPart>, pos: Vec3, yaw: f32| v.push(Piece { parts, pos, yaw });
    // South wall in two runs either side of the gatehouse (outer face −Z: yaw 0).
    let gate_w = 16.0;
    let s_len = cx - r * 0.86 - (gate_w * 0.5 - 1.0);
    let s_mid = (cx - r * 0.86 + gate_w * 0.5 - 1.0) * 0.5;
    put(
        architecture::curtain_wall(s_len, h, t, 1),
        Vec3::new(-s_mid, 0.0, -cz),
        0.0,
    );
    put(
        architecture::curtain_wall(s_len, h, t, 2),
        Vec3::new(s_mid, 0.0, -cz),
        0.0,
    );
    put(
        architecture::gatehouse(gate_w, h + 3.5, 10.0, 3),
        Vec3::new(0.0, 0.0, -cz + 1.5),
        0.0,
    );
    // North wall with a breach in the middle (outer face +Z: yaw π).
    let n_len = cx - r * 0.86 - 7.0;
    let n_mid = (cx - r * 0.86 + 7.0) * 0.5;
    put(
        architecture::curtain_wall(n_len, h, t, 4),
        Vec3::new(-n_mid, 0.0, cz),
        PI,
    );
    put(
        architecture::curtain_wall(n_len, h, t, 5),
        Vec3::new(n_mid, 0.0, cz),
        PI,
    );
    put(
        architecture::ruined_wall(14.0, h * 0.9, 6),
        Vec3::new(0.0, 0.0, cz),
        PI,
    );
    // East and west walls (outer faces +X and −X).
    let e_len = 2.0 * (cz - r * 0.86);
    put(
        architecture::curtain_wall(e_len, h, t, 7),
        Vec3::new(cx, 0.0, 0.0),
        -FRAC_PI_2,
    );
    put(
        architecture::curtain_wall(e_len, h, t, 8),
        Vec3::new(-cx, 0.0, 0.0),
        FRAC_PI_2,
    );
    // Corner towers.
    put(
        architecture::round_tower_with(r, 14.0, TowerTop::SlateRoof, 9),
        Vec3::new(-cx, 0.0, -cz),
        0.0,
    );
    put(
        architecture::round_tower_with(r, 13.5, TowerTop::Crenellated, 10),
        Vec3::new(cx, 0.0, -cz),
        0.0,
    );
    put(
        architecture::round_tower_with(r, 13.0, TowerTop::Hoarding, 11),
        Vec3::new(cx, 0.0, cz),
        0.0,
    );
    put(
        architecture::square_tower(9.0, 15.0, 12),
        Vec3::new(-cx, 0.0, cz),
        0.0,
    );
    v
}

fn setup(
    mut commands: Commands,
    fortress: Res<Fortress>,
    tex: Res<Textured>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
) {
    let mut cache: Vec<(DestructibleMaterial, StandardMaterial)> = Vec::new();
    let mut material = |m: DestructibleMaterial, images: &mut Assets<Image>| {
        if !tex.0 {
            return color(m);
        }
        if let Some((_, s)) = cache.iter().find(|(k, _)| *k == m) {
            return *s;
        }
        let s = textured(texture_kind(m), images);
        cache.push((m, s));
        s
    };
    let (grass, yard) = if tex.0 {
        (
            textured(SurfaceKind::Grass, &mut images),
            textured(SurfaceKind::Dirt, &mut images),
        )
    } else {
        (
            StandardMaterial::hex(0x56663a).with_roughness(0.95),
            StandardMaterial::hex(0x8c8270).with_roughness(0.95),
        )
    };
    commands.spawn((
        Camera {
            exposure: Exposure::Manual { ev100: 14.2 },
            ..Camera::perspective(55.0)
        },
        Transform::from_xyz(-80.0, 40.0, -90.0).looking_at(DVec3::ZERO, Vec3::Y),
    ));
    // Low late-afternoon sun from the south-west so the outer faces show their relief.
    commands.spawn((
        DirectionalLight {
            color: [1.0, 0.88, 0.72],
            illuminance: 75_000.0,
            ..Default::default()
        },
        Transform::default().looking_at(DVec3::new(0.8, -0.32, 0.5), Vec3::Y),
    ));
    commands.spawn((
        Transform::from_xyz(0.0, 0.0, 0.0),
        MeshRenderer::new(
            Shape::Plane {
                size: [4000.0, 4000.0],
                subdivisions: 1,
            },
            grass,
        ),
    ));
    commands.spawn((
        Transform::from_xyz(0.0, 0.01, 0.0),
        MeshRenderer::new(
            Shape::Plane {
                size: [72.0, 52.0],
                subdivisions: 1,
            },
            yard,
        ),
    ));
    for piece in &fortress.0 {
        for part in &piece.parts {
            let h = meshes.add(part.mesh.clone().build());
            let mut t = Transform::from_translation(piece.pos.as_dvec3());
            t.rotation = Quat::from_rotation_y(piece.yaw);
            let mat = material(part.material, &mut images);
            commands.spawn((t, MeshRenderer::new(MeshRef::Handle(h), mat)));
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let out = PathBuf::from(
        args.get(1)
            .cloned()
            .unwrap_or_else(|| "architecture_shots".into()),
    );
    let (w, h) = args
        .get(2)
        .and_then(|s| s.split_once('x'))
        .and_then(|(a, b)| Some((a.parse().ok()?, b.parse().ok()?)))
        .unwrap_or((1280u32, 720u32));
    let tex = args.iter().any(|a| a == "--textured");
    let args: Vec<String> = args.into_iter().filter(|a| a != "--textured").collect();
    let only = args.get(3).cloned();
    std::fs::create_dir_all(&out).expect("create output dir");

    let start = Instant::now();
    let pieces = build();
    let tris: usize = pieces
        .iter()
        .map(|p| architecture::triangle_count(&p.parts))
        .sum();
    println!(
        "built {} pieces, {} triangles in {:.1} s",
        pieces.len(),
        tris,
        start.elapsed().as_secs_f32()
    );
    for (i, p) in pieces.iter().enumerate() {
        println!(
            "  piece {i}: {} triangles",
            architecture::triangle_count(&p.parts)
        );
    }

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
    app.add_plugins(RenderPlugin)
        .insert_resource(Fortress(pieces))
        .insert_resource(Textured(tex))
        .add_systems(Startup, setup);

    // Run the startup systems so the camera exists before the first shot is framed.
    app.update();
    for (name, eye, target, fov) in shots() {
        if only.as_deref().is_some_and(|o| !name.contains(o)) {
            continue;
        }
        {
            let mut q = app.world.query::<(&mut Transform, &mut Camera)>();
            for (mut t, mut c) in q.iter_mut(&mut app.world) {
                *t = Transform::from_translation(DVec3::from_array(eye))
                    .looking_at(DVec3::from_array(target), Vec3::Y);
                if let Projection::Perspective { fov_y_deg, .. } = &mut c.projection {
                    *fov_y_deg = fov;
                }
            }
        }
        let t0 = Instant::now();
        app.update_n(3);
        if let Some(d) = app.world.get_resource::<RenderDisabled>() {
            eprintln!("no GPU: {}", d.0);
            return;
        }
        let path = out.join(format!("{name}{}.png", if tex { "_tex" } else { "" }));
        match capture_now(&mut app.world, &path) {
            Ok(_) => println!("{} ({:.1} s)", path.display(), t0.elapsed().as_secs_f32()),
            Err(e) => eprintln!("{name}: {e}"),
        }
    }
}
