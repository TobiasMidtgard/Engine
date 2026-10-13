//! Preview harness for the proving ground's prop meshes (`src/demo/art/props.rs`): prints
//! every prop's triangle count and renders them with the engine, headless, under a low side
//! sun so bevels catch highlights: a lineup and a close-up of each prop.
//!
//! ```sh
//! cargo run -p engine_physics --example art_props -- <out_dir> [filter] [width]x[height]
//! ```
//!
//! `filter` renders only the shots whose name contains it. Rendering needs a GPU or a
//! software Vulkan driver (lavapipe: `apt-get install mesa-vulkan-drivers`).

#[allow(dead_code)]
#[path = "../src/demo/art/kit.rs"]
mod kit;
#[path = "../src/demo/art/props.rs"]
mod props;

use std::f32::consts::FRAC_PI_2;
use std::marker::PhantomData;
use std::path::{Path, PathBuf};

use bevy_ecs::component::Mutable;
use engine_core::math::{DVec3, Quat, Vec3};
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, capture_now};

/// `engine_assets` is not a dependency of this crate, so the harness names `Assets<Mesh>`
/// through the renderer's own system that takes it (as its third parameter).
trait ThirdParam<M> {
    type Out;
}

impl<F, A, B, C, D, E> ThirdParam<(A, B, C, D, E)> for F
where
    F: FnMut(A, B, C, D, E),
{
    type Out = C;
}

fn third_param<M, F: ThirdParam<M>>(_: &F) -> PhantomData<F::Out> {
    PhantomData
}

fn resource<'w, T: Resource<Mutability = Mutable>>(
    world: &'w mut World,
    _: PhantomData<ResMut<'_, T>>,
) -> Mut<'w, T> {
    world.resource_mut::<T>()
}

fn add_mesh(world: &mut World, mesh: Mesh) -> MeshRef {
    let meshes = third_param(&engine_render::resolve_mesh_renderers);
    resource(world, meshes).add(mesh).into()
}

fn wood() -> StandardMaterial {
    StandardMaterial::hex(0xb08050).with_roughness(0.8)
}

fn painted_steel(hex: u32) -> StandardMaterial {
    StandardMaterial::hex(hex).with_roughness(0.5)
}

fn forged_steel() -> StandardMaterial {
    StandardMaterial::hex(0x8a8d92)
        .with_metallic(1.0)
        .with_roughness(0.45)
}

fn bracket_steel() -> StandardMaterial {
    StandardMaterial::hex(0x7d8187)
        .with_metallic(1.0)
        .with_roughness(0.5)
}

fn rubber(hex: u32) -> StandardMaterial {
    StandardMaterial::hex(hex).with_roughness(0.55)
}

fn resin() -> StandardMaterial {
    StandardMaterial::hex(0xffffff).with_roughness(0.3)
}

struct Scene<'a> {
    world: &'a mut World,
}

impl Scene<'_> {
    fn put(&mut self, name: &str, mesh: Mesh, material: StandardMaterial, at: Vec3, rot: Quat) {
        println!("{name:<34} {:>6} triangles", mesh.triangle_count());
        let mesh = add_mesh(self.world, mesh);
        self.world.spawn((
            Transform::from_translation(at.as_dvec3()).with_rotation(rot),
            MeshRenderer::new(mesh, material),
        ));
    }
}

/// (file name, camera position, look-at target).
type Shot = (&'static str, [f32; 3], [f32; 3]);

const SHOTS: &[Shot] = &[
    (
        "01_lineup_wood_and_steel",
        [2.2, 2.3, 6.4],
        [2.0, 0.45, 0.0],
    ),
    (
        "01_lineup_balls_and_steel",
        [9.6, 2.4, 7.2],
        [9.7, 0.85, 0.0],
    ),
    ("02_crate_small", [0.65, 0.7, 1.05], [0.0, 0.18, 0.0]),
    ("03_crate_large", [2.35, 1.45, 1.85], [1.4, 0.4, 0.0]),
    ("04_crate_oblong", [3.75, 1.0, 1.35], [3.0, 0.18, 0.0]),
    ("05_crate_stack", [-0.6, 1.5, 2.1], [-1.4, 0.6, 0.0]),
    ("06_drums", [5.7, 1.35, 2.25], [4.85, 0.45, 0.25]),
    ("07_drum_lid", [4.65, 1.5, 0.7], [4.4, 0.8, 0.0]),
    ("08_balls", [7.15, 0.95, 2.0], [6.9, 0.3, 0.25]),
    ("09_wrecking_ball", [10.9, 2.1, 3.6], [9.4, 1.4, 0.0]),
    ("10_shackle", [10.15, 1.98, 0.42], [9.4, 1.78, 0.0]),
    ("11_chain", [9.95, 2.7, 0.8], [9.4, 2.45, 0.0]),
    ("12_dominoes", [12.95, 1.05, 2.3], [12.4, 0.45, 0.0]),
    ("13_domino_face", [12.25, 0.8, 0.85], [12.0, 0.5, 0.0]),
];

/// Re-encode a screenshot as JPEG (previews stay well under 1 MB) and drop the PNG.
fn to_jpeg(png: &Path) -> Result<PathBuf, String> {
    let img = image::open(png).map_err(|e| e.to_string())?.to_rgb8();
    let jpg = png.with_extension("jpg");
    let file = std::fs::File::create(&jpg).map_err(|e| e.to_string())?;
    image::codecs::jpeg::JpegEncoder::new_with_quality(std::io::BufWriter::new(file), 92)
        .encode_image(&img)
        .map_err(|e| e.to_string())?;
    std::fs::remove_file(png).map_err(|e| e.to_string())?;
    Ok(jpg)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let out = PathBuf::from(args.get(1).cloned().unwrap_or("art_props".into()));
    let only = args.get(2).cloned().unwrap_or_default();
    let (w, h) = args
        .get(3)
        .and_then(|s| s.split_once('x'))
        .and_then(|(a, b)| Some((a.parse().ok()?, b.parse().ok()?)))
        .unwrap_or((1280u32, 720u32));
    std::fs::create_dir_all(&out).expect("output directory");

    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut win = app.world.resource_mut::<Window>();
        win.width = w;
        win.height = h;
    }
    app.add_plugins(RenderPlugin);
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 60.0,
        });
    app.world.resource_mut::<AmbientLight>().brightness = 400.0;
    let camera = app
        .world
        .spawn((
            Camera::perspective(40.0).with_exposure(Exposure::Manual { ev100: 14.3 }),
            Transform::from_xyz(0.0, 2.0, 6.0),
        ))
        .id();
    // Sun from the front left: lit fronts and tops, shaded right sides, so edges read.
    app.world.spawn((
        DirectionalLight::default(),
        Transform::default().looking_at(DVec3::new(0.55, -0.6, -0.58), Vec3::Y),
    ));
    app.world.spawn((
        Transform::from_xyz(0.0, 0.0, 0.0),
        MeshRenderer::new(
            Shape::Plane {
                size: [80.0, 80.0],
                subdivisions: 1,
            },
            StandardMaterial::hex(0x8c8b86).with_roughness(0.9),
        ),
    ));

    let mut s = Scene {
        world: &mut app.world,
    };
    let y = Quat::from_rotation_y;
    for (name, half, seed, at, rot) in [
        (
            "crate 0.22",
            Vec3::splat(0.22),
            1u32,
            Vec3::new(0.0, 0.22, 0.0),
            0.45f32,
        ),
        (
            "crate 0.45",
            Vec3::splat(0.45),
            2,
            Vec3::new(1.4, 0.45, 0.0),
            -0.35,
        ),
        (
            "crate 0.45 x 0.2 x 0.3",
            Vec3::new(0.45, 0.2, 0.3),
            3,
            Vec3::new(3.0, 0.2, 0.0),
            0.25,
        ),
        (
            "crate 0.3 (stack, bottom)",
            Vec3::splat(0.3),
            4,
            Vec3::new(-1.4, 0.3, 0.0),
            0.1,
        ),
        (
            "crate 0.3 (stack, top)",
            Vec3::splat(0.3),
            5,
            Vec3::new(-1.38, 0.9, 0.02),
            0.32,
        ),
    ] {
        let c = props::crate_box(half, seed);
        s.put(&format!("{name} wood"), c.wood, wood(), at, y(rot));
        s.put(
            &format!("{name} metal"),
            c.metal,
            bracket_steel(),
            at,
            y(rot),
        );
    }
    s.put(
        "steel drum",
        props::steel_drum(0.3, 0.42),
        painted_steel(0x2f5f9e),
        Vec3::new(4.4, 0.42, 0.0),
        y(0.3),
    );
    s.put(
        "steel drum (lying)",
        props::steel_drum(0.3, 0.42),
        painted_steel(0xb8392f),
        Vec3::new(5.25, 0.3, 0.75),
        Quat::from_rotation_x(FRAC_PI_2) * y(0.6),
    );
    s.put(
        "ball r 0.3",
        props::ball(0.3),
        rubber(0xd8c23a),
        Vec3::new(6.4, 0.3, 0.0),
        y(0.0),
    );
    s.put(
        "playground ball r 0.35",
        props::playground_ball(0.35),
        rubber(0xc8402f),
        Vec3::new(7.25, 0.35, 0.1),
        Quat::from_rotation_x(0.35) * y(0.5),
    );
    s.put(
        "playground ball r 0.25",
        props::playground_ball(0.25),
        rubber(0x3a8fe0),
        Vec3::new(6.85, 0.25, 0.85),
        Quat::from_rotation_z(0.6),
    );
    let ball_at = Vec3::new(9.4, 0.8, 0.0);
    s.put(
        "wrecking ball r 0.8",
        props::wrecking_ball(0.8),
        forged_steel(),
        ball_at,
        y(0.0),
    );
    let link = props::CHAIN_SEGMENT_LENGTH;
    for i in 0..3 {
        let mesh = if i == 0 {
            props::chain_end_segment(link, props::CHAIN_WIRE)
        } else {
            props::chain_segment(link, props::CHAIN_WIRE)
        };
        let at = ball_at + Vec3::Y * (0.8 + link * (i as f32 + 0.5));
        s.put(
            &format!("chain segment {i}"),
            mesh,
            forged_steel(),
            at,
            y(0.0),
        );
    }
    let half = Vec3::new(0.25, 0.5, 0.06);
    s.put(
        "domino 6|3 ivory",
        props::domino(half, (6, 3), true),
        resin(),
        Vec3::new(12.0, 0.5, 0.0),
        y(0.0),
    );
    s.put(
        "domino 5|2 black",
        props::domino(half, (5, 2), false),
        resin(),
        Vec3::new(12.75, 0.5, -0.2),
        y(-0.35),
    );
    s.put(
        "domino 6|6 black (lying)",
        props::domino(half, (6, 6), false),
        resin(),
        Vec3::new(12.6, 0.06, 0.75),
        Quat::from_rotation_x(-FRAC_PI_2) * y(0.0),
    );

    for (name, eye, target) in SHOTS.iter().filter(|s| s.0.contains(only.as_str())) {
        {
            let mut tr = app.world.get_mut::<Transform>(camera).unwrap();
            *tr = Transform::from_translation(Vec3::from(*eye).as_dvec3())
                .looking_at(Vec3::from(*target).as_dvec3(), Vec3::Y);
        }
        for _ in 0..3 {
            app.update();
        }
        if let Some(d) = app.world.get_resource::<RenderDisabled>() {
            eprintln!("no GPU: {}", d.0);
            return;
        }
        let path = out.join(format!("{name}.png"));
        match capture_now(&mut app.world, &path).and_then(|_| to_jpeg(&path)) {
            Ok(jpg) => println!("{}", jpg.display()),
            Err(e) => eprintln!("{name}: {e}"),
        }
    }
}
