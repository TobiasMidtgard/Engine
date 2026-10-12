//! `cargo run -p hello` opens a window; `cargo run -p hello -- --headless --frames 3`
//! renders offscreen and writes `screenshots/hello.png`.

use engine::prelude::*;

fn main() {
    let mut app = App::new();
    engine::core::log::init("info,wgpu=warn,naga=warn");
    engine::apply_cli_args(&mut app);
    app.add_plugins(DefaultPlugins)
        .add_systems(Startup, setup)
        .add_systems(Update, (fly_camera, screenshot_when_headless));
    app.run();
}

#[derive(Component)]
struct FlyCamera {
    yaw: f32,
    pitch: f32,
}

fn setup(mut commands: Commands) {
    commands.spawn((
        Camera::default(),
        FlyCamera {
            yaw: 0.0,
            pitch: -0.15,
        },
        Transform::from_xyz(0.0, 2.0, 8.0),
    ));
    commands.spawn((
        DirectionalLight::default(),
        Transform::default().looking_at(DVec3::new(-0.5, -0.7, -0.4), Vec3::Y),
    ));
    commands.spawn((
        Transform::default(),
        MeshRenderer::new(
            Shape::Plane {
                size: [200.0, 200.0],
                subdivisions: 1,
            },
            StandardMaterial::hex(0x5a7d3a).with_roughness(0.95),
        ),
    ));
    let colors = [0xd94f30, 0xe0b040, 0x3a9ad9, 0x8f5bd9, 0x40c070];
    for (i, c) in colors.iter().enumerate() {
        let x = i as f64 * 2.5 - 5.0;
        commands.spawn((
            Transform::from_xyz(x, 1.0, 0.0),
            MeshRenderer::new(
                Shape::Sphere {
                    radius: 1.0,
                    segments: 48,
                },
                StandardMaterial::hex(*c).with_roughness(0.2 + i as f32 * 0.15),
            ),
        ));
    }
}

fn fly_camera(
    actions: Res<ActionState>,
    time: Res<Time>,
    mut window: ResMut<engine::core::window::Window>,
    mouse: Res<ButtonInput<MouseButton>>,
    mut q: Query<(&mut Transform, &mut FlyCamera)>,
) {
    if mouse.just_pressed(MouseButton::Right) {
        window.cursor_mode = CursorMode::Locked;
    }
    if mouse.just_released(MouseButton::Right) {
        window.cursor_mode = CursorMode::Normal;
    }
    let dt = time.real_delta_secs();
    for (mut t, mut cam) in &mut q {
        if window.cursor_mode == CursorMode::Locked {
            let look = actions.axis("look");
            cam.yaw -= look.x * 0.003;
            cam.pitch = (cam.pitch + look.y * 0.003).clamp(-1.5, 1.5);
        }
        t.rotation = Quat::from_rotation_y(cam.yaw) * Quat::from_rotation_x(cam.pitch);
        let m = actions.axis("move");
        let speed = if actions.pressed("sprint") { 30.0 } else { 6.0 };
        let delta = t.forward() * m.y + t.right() * m.x + Vec3::Y * actions.value("ascend");
        t.translation += (delta * speed * dt).as_dvec3();
    }
}

fn screenshot_when_headless(
    config: Res<EngineConfig>,
    time: Res<Time>,
    mut shots: ResMut<Screenshots>,
    mut taken: Local<bool>,
) {
    if config.headless && !*taken && time.frame_count() >= 2 {
        shots.request("screenshots/hello.png");
        *taken = true;
    }
}
