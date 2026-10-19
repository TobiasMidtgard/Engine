//! Depth of field and motion blur, before and after.
//!
//! - A courtyard at dusk through an 85 mm lens: a marble sphere on a plinth, a rusty post in the
//!   foreground and string lights on a brick wall behind. Without depth of field, at f/1.4 with
//!   autofocus (the lights open into discs, the post melts over the subject), focused on the
//!   wall instead, and stopped down to f/11.
//! - A whip pan across a street of lamp posts, and a tracking shot following a car at 15 m/s
//!   (the car stays sharp, the street streaks behind it), each without and with motion blur
//!   (a 180° shutter at 30 fps: 1/60 s).
//!
//! ```sh
//! cargo run -p engine_render --example dof_motion_blur -- [out-dir]
//! ```

use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, Screenshots, capture_now};

const WIDTH: u32 = 1280;
const HEIGHT: u32 = 720;
const DT: f64 = 1.0 / 60.0;

fn base_app() -> App {
    engine_core::log::init("warn");
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = WIDTH;
        w.height = HEIGHT;
    }
    app.add_plugins(RenderPlugin);
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual { step_secs: DT });
    {
        let mut s = app.world.resource_mut::<RenderSettings>();
        s.material_texture_size = 512;
        s.film_grain = 0.0;
    }
    app.add_systems(Update, (spin, drive, track).chain());
    app
}

/// Turns its entity about the vertical axis (rad/s of world time).
#[derive(Component)]
struct Spin(f32);

/// Moves its entity (m/s of world time).
#[derive(Component)]
struct Drive(DVec3);

/// Keeps the camera pointed at an entity.
#[derive(Component)]
struct Track(Entity, DVec3);

fn spin(time: Res<Time>, mut q: Query<(&Spin, &mut Transform)>) {
    for (spin, mut t) in &mut q {
        t.rotation = Quat::from_rotation_y(spin.0 * time.delta_secs()) * t.rotation;
    }
}

fn drive(time: Res<Time>, mut q: Query<(&Drive, &mut Transform)>) {
    for (drive, mut t) in &mut q {
        t.translation += drive.0 * time.delta_secs_f64();
    }
}

fn track(mut cameras: Query<(&Track, &mut Transform)>, targets: Query<&Transform, Without<Track>>) {
    for (track, mut t) in &mut cameras {
        if let Ok(target) = targets.get(track.0) {
            let at = target.translation + track.1;
            *t = Transform::from_translation(t.translation).looking_at(at, Vec3::Y);
        }
    }
}

fn spawn(app: &mut App, at: DVec3, shape: Shape, material: StandardMaterial) -> Entity {
    app.world
        .spawn((
            Transform::from_translation(at),
            MeshRenderer::new(shape, material),
        ))
        .id()
}

fn bulb(app: &mut App, at: DVec3, color: [f32; 3], nits: f32) {
    app.world.spawn((
        Transform::from_translation(at),
        MeshRenderer::new(
            Shape::Sphere {
                radius: 0.06,
                segments: 12,
            },
            StandardMaterial::color(0.0, 0.0, 0.0).with_emissive(color, nits),
        )
        .without_shadows(),
    ));
}

/// The courtyard at dusk. Returns the camera.
fn courtyard(app: &mut App) -> Entity {
    let fov = DepthOfField::fov_y_deg_for_lens(85.0, 24.0);
    let camera = app
        .world
        .spawn((
            Camera {
                exposure: Exposure::Manual { ev100: 7.5 },
                ..Camera::perspective(fov)
            },
            Transform::from_xyz(0.35, 1.25, 4.6).looking_at(DVec3::new(0.0, 1.05, 0.0), Vec3::Y),
        ))
        .id();
    app.world.spawn((
        DirectionalLight {
            illuminance: 4000.0,
            ..Default::default()
        },
        Transform::default().looking_at(DVec3::new(-0.6, -0.08, -0.5), Vec3::Y),
    ));
    let ground = StandardMaterial::preset(MaterialPreset::Cobblestone).with_texture_scale(1.6);
    spawn(
        app,
        DVec3::ZERO,
        Shape::Plane {
            size: [120.0, 120.0],
            subdivisions: 4,
        },
        ground,
    );
    // The subject: a marble sphere on a concrete plinth.
    spawn(
        app,
        DVec3::new(0.0, 0.45, 0.0),
        Shape::RoundedCuboid {
            size: [0.55, 0.9, 0.55],
            radius: 0.03,
            segments: 4,
        },
        StandardMaterial::preset(MaterialPreset::Concrete).with_texture_scale(1.0),
    );
    spawn(
        app,
        DVec3::new(0.0, 1.2, 0.0),
        Shape::Sphere {
            radius: 0.3,
            segments: 0,
        },
        StandardMaterial::preset(MaterialPreset::Marble).with_texture_scale(0.8),
    );
    // A rusty post right in front of the lens, and a rock beside the path.
    spawn(
        app,
        DVec3::new(0.08, 1.0, 3.3),
        Shape::Cylinder {
            radius: 0.05,
            height: 2.0,
            segments: 0,
        },
        StandardMaterial::preset(MaterialPreset::RustySteel).with_texture_scale(0.5),
    );
    spawn(
        app,
        DVec3::new(-1.6, 0.0, -3.0),
        Shape::Rock {
            size: [1.2, 0.8, 1.0],
            seed: 7,
            detail: 0,
        },
        StandardMaterial::preset(MaterialPreset::Rock).with_texture_scale(1.5),
    );
    // Barrels in the middle distance.
    for (i, (x, z)) in [(1.6, -6.0), (2.3, -6.6), (-2.6, -10.0)]
        .into_iter()
        .enumerate()
    {
        spawn(
            app,
            DVec3::new(x, 0.45, z),
            Shape::Cylinder {
                radius: 0.3,
                height: 0.9,
                segments: 0,
            },
            StandardMaterial::preset(MaterialPreset::PaintedMetal)
                .with_tint_hex([0x2f5d8a, 0x8a2f2f, 0x3d6b3a][i]),
        );
    }
    // The brick wall and its string lights.
    spawn(
        app,
        DVec3::new(0.0, 3.0, -24.0),
        Shape::Cuboid {
            size: [60.0, 6.0, 0.5],
        },
        StandardMaterial::preset(MaterialPreset::Brick)
            .with_texture_scale(2.0)
            .with_triplanar_world(),
    );
    let warm = [1.0, 0.62, 0.3];
    for strand in 0..3 {
        let (z, y, sag) = (
            -23.4 + strand as f64 * 0.2,
            4.6 - strand as f64 * 0.9,
            1.2 + strand as f64 * 0.3,
        );
        for i in 0..36 {
            let s = i as f64 / 35.0;
            let x = -14.0 + 28.0 * s + strand as f64 * 0.4;
            let h = y - sag * 4.0 * s * (1.0 - s);
            bulb(app, DVec3::new(x, h, z), warm, 60_000.0);
        }
    }
    // A few colored lanterns.
    for (x, y, color) in [
        (-6.5, 2.2, [0.3, 0.6, 1.0]),
        (4.5, 2.6, [1.0, 0.25, 0.2]),
        (8.0, 1.6, [0.4, 1.0, 0.5]),
    ] {
        bulb(app, DVec3::new(x, y, -23.0), color, 120_000.0);
        app.world.spawn((
            PointLight {
                color,
                intensity_lm: 3000.0,
                range: 12.0,
                ..Default::default()
            },
            Transform::from_xyz(x, y, -22.5),
        ));
    }
    // Lamps over the courtyard.
    for (x, z) in [(-2.5, 1.5), (2.0, -2.0)] {
        app.world.spawn((
            PointLight {
                color: warm,
                intensity_lm: 2500.0,
                range: 10.0,
                ..Default::default()
            },
            Transform::from_xyz(x, 2.6, z),
        ));
    }
    camera
}

/// A street of lamp posts in front of a brick wall, with a car. Returns the camera and the car.
fn street(app: &mut App) -> (Entity, Entity) {
    let camera = app
        .world
        .spawn((
            Camera {
                exposure: Exposure::Manual { ev100: 11.0 },
                ..Camera::perspective(55.0)
            },
            Transform::from_xyz(0.0, 1.6, 6.0).looking_at(DVec3::new(0.0, 1.4, -6.0), Vec3::Y),
        ))
        .id();
    app.world.spawn((
        DirectionalLight {
            illuminance: 30_000.0,
            ..Default::default()
        },
        Transform::default().looking_at(DVec3::new(-0.5, -0.35, -0.6), Vec3::Y),
    ));
    spawn(
        app,
        DVec3::ZERO,
        Shape::Plane {
            size: [300.0, 300.0],
            subdivisions: 4,
        },
        StandardMaterial::preset(MaterialPreset::Cobblestone).with_texture_scale(1.6),
    );
    spawn(
        app,
        DVec3::new(0.0, 4.0, -14.0),
        Shape::Cuboid {
            size: [160.0, 8.0, 0.5],
        },
        StandardMaterial::preset(MaterialPreset::Brick)
            .with_texture_scale(2.0)
            .with_triplanar_world(),
    );
    let pole = StandardMaterial::preset(MaterialPreset::PaintedMetal).with_tint_hex(0x1f2a24);
    for i in -12..=12 {
        let x = i as f64 * 4.0;
        spawn(
            app,
            DVec3::new(x, 2.0, -11.0),
            Shape::Cylinder {
                radius: 0.08,
                height: 4.0,
                segments: 0,
            },
            pole,
        );
        bulb(
            app,
            DVec3::new(x, 4.05, -11.0),
            [1.0, 0.75, 0.45],
            400_000.0,
        );
        // Posters on the wall.
        spawn(
            app,
            DVec3::new(x + 2.0, 2.2, -13.7),
            Shape::Cuboid {
                size: [1.2, 1.7, 0.05],
            },
            StandardMaterial::hex([0xd9a43a, 0x3a7fd9, 0xd94f3a, 0xeeeeee][(i + 12) as usize % 4])
                .with_roughness(0.5),
        );
    }
    // The car: a red body on four wheels.
    let car = app.world.spawn(Transform::from_xyz(-30.0, 0.0, -5.0)).id();
    let body = StandardMaterial::preset(MaterialPreset::PaintedMetal).with_tint_hex(0xb3241d);
    let tire = StandardMaterial::color(0.03, 0.03, 0.03).with_roughness(0.8);
    let parts = [
        (
            DVec3::new(0.0, 0.75, 0.0),
            Shape::RoundedCuboid {
                size: [4.2, 0.9, 1.8],
                radius: 0.2,
                segments: 4,
            },
            body,
        ),
        (
            DVec3::new(-0.3, 1.45, 0.0),
            Shape::RoundedCuboid {
                size: [2.2, 0.6, 1.6],
                radius: 0.2,
                segments: 4,
            },
            body,
        ),
    ];
    for (offset, shape, material) in parts {
        let part = spawn(app, offset, shape, material);
        app.world.entity_mut(part).insert(ChildOf(car));
    }
    for (x, z) in [(-1.4, 0.95), (1.4, 0.95), (-1.4, -0.95), (1.4, -0.95)] {
        app.world.spawn((
            Transform::from_xyz(x, 0.38, z)
                .with_rotation(Quat::from_rotation_x(std::f32::consts::FRAC_PI_2)),
            MeshRenderer::new(
                Shape::Cylinder {
                    radius: 0.38,
                    height: 0.25,
                    segments: 0,
                },
                tire,
            ),
            ChildOf(car),
        ));
    }
    (camera, car)
}

fn shot(app: &mut App, out: &std::path::Path, name: &str) {
    let path = out.join(format!("{name}.png"));
    capture_now(&mut app.world, &path).expect("screenshot");
    println!("{}", path.display());
}

/// Capture the frame the next `update` renders (motion blur needs the frame's own motion).
fn shot_while_moving(app: &mut App, out: &std::path::Path, name: &str) {
    let path = out.join(format!("{name}.png"));
    app.world.resource_mut::<Screenshots>().request(&path);
    app.update();
    println!("{}", path.display());
}

fn main() {
    let out = std::path::PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| {
        "/mnt/project-files/engine-screenshots/rendering/round7/dof_motion_blur".into()
    }));
    // Optional: only "dof" or only "motion_blur".
    let only = std::env::args().nth(2).unwrap_or_default();
    std::fs::create_dir_all(&out).expect("output directory");
    if only != "motion_blur" {
        depth_of_field(&out);
    }
    if only != "dof" {
        motion_blur(&out);
    }
}

fn depth_of_field(out: &std::path::Path) {
    let out = out.to_path_buf();

    // Depth of field.
    let mut app = base_app();
    let camera = courtyard(&mut app);
    app.update_n(24);
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("no GPU: {}", d.0);
        return;
    }
    shot(&mut app, &out, "dof_off");
    app.world
        .entity_mut(camera)
        .insert(DepthOfField::autofocus(1.4));
    app.update_n(24);
    shot(&mut app, &out, "dof_f1.4_autofocus");
    if let Some(focus) = app
        .world
        .resource::<Diagnostics>()
        .get("render.dof_focus_m")
    {
        println!("autofocus at {focus:.2} m");
    }
    app.world
        .entity_mut(camera)
        .insert(DepthOfField::new(24.0, 1.4));
    app.update_n(8);
    shot(&mut app, &out, "dof_f1.4_focus_wall");
    app.world
        .entity_mut(camera)
        .insert(DepthOfField::autofocus(11.0));
    app.update_n(16);
    shot(&mut app, &out, "dof_f11_autofocus");
}

/// A whip pan (120°/s) and a tracking shot.
fn motion_blur(out: &std::path::Path) {
    let out = out.to_path_buf();
    for blur in [false, true] {
        let suffix = if blur { "motion_blur" } else { "no_blur" };
        let mut app = base_app();
        let (camera, _) = street(&mut app);
        app.update_n(16);
        app.world
            .entity_mut(camera)
            .insert(Spin(-120f32.to_radians()));
        if blur {
            app.world.entity_mut(camera).insert(MotionBlur::default());
        }
        app.update_n(6);
        shot_while_moving(&mut app, &out, &format!("pan_{suffix}"));

        let mut app = base_app();
        let (camera, car) = street(&mut app);
        app.world
            .entity_mut(car)
            .insert(Drive(DVec3::new(15.0, 0.0, 0.0)));
        app.world
            .entity_mut(camera)
            .insert(Track(car, DVec3::new(0.0, 1.0, 0.0)));
        if blur {
            app.world.entity_mut(camera).insert((
                MotionBlur::default(),
                DepthOfField::autofocus(2.0).with_sensor_height_mm(24.0),
            ));
        }
        app.update_n(110);
        shot_while_moving(&mut app, &out, &format!("tracking_{suffix}"));
    }
}
