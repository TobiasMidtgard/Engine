//! A small lit scene with the AI bridge always on, for trying the protocol and `engine-mcp`.
//!
//! ```sh
//! cargo run -p engine_ai_bridge --example bridge_demo -- --headless
//! ENGINE_BRIDGE=127.0.0.1:9000 cargo run -p engine_ai_bridge --example bridge_demo
//! ```
//!
//! Without `ENGINE_BRIDGE` it listens on 127.0.0.1:7878. Flags: `--headless` (no window,
//! offscreen rendering), `--size WxH`.

use engine_ai_bridge::AiBridgePlugin;
use engine_core::prelude::*;
use engine_render::prelude::*;

fn main() {
    engine_core::log::init("info,wgpu=warn,naga=warn");
    let mut app = App::new();
    {
        let mut config = app.world.resource_mut::<EngineConfig>();
        config.app_name = "Bridge demo".into();
        config.width = 960;
        config.height = 540;
        let args: Vec<String> = std::env::args().collect();
        config.headless = args.iter().any(|a| a == "--headless");
        if let Some(size) = args
            .iter()
            .position(|a| a == "--size")
            .and_then(|i| args.get(i + 1))
            && let Some((w, h)) = size.split_once('x')
        {
            config.width = w.parse().unwrap_or(config.width);
            config.height = h.parse().unwrap_or(config.height);
        }
    }
    let (width, height) = {
        let c = app.world.resource::<EngineConfig>();
        (c.width, c.height)
    };
    {
        let mut window = app.world.resource_mut::<Window>();
        window.width = width;
        window.height = height;
    }
    app.add_plugins((
        engine_input::InputPlugin,
        RenderPlugin,
        engine_platform::PlatformPlugin,
        AiBridgePlugin::listen(engine_ai_bridge::DEFAULT_ADDRESS),
    ))
    .add_systems(Startup, setup);
    app.run();
}

fn setup(mut commands: Commands) {
    commands.spawn((
        Name::new("Camera"),
        Camera::default(),
        Transform::from_xyz(0.0, 2.0, 8.0).looking_at(DVec3::new(0.0, 1.0, 0.0), Vec3::Y),
    ));
    commands.spawn((
        Name::new("Sun"),
        DirectionalLight::default(),
        Transform::default().looking_at(DVec3::new(-0.5, -0.7, -0.4), Vec3::Y),
    ));
    commands.spawn((
        Name::new("Ground"),
        Transform::default(),
        MeshRenderer::new(
            Shape::Plane {
                size: [200.0, 200.0],
                subdivisions: 1,
            },
            StandardMaterial::hex(0x5a7d3a).with_roughness(0.95),
        ),
    ));
    let colors = [0xd94f30, 0xe0b040, 0x3a9ad9];
    for (i, color) in colors.iter().enumerate() {
        commands.spawn((
            Name::new(format!("Ball {i}")),
            Transform::from_xyz(i as f64 * 2.5 - 2.5, 1.0, 0.0),
            MeshRenderer::new(
                Shape::Sphere {
                    radius: 1.0,
                    segments: 48,
                },
                StandardMaterial::hex(*color).with_roughness(0.3),
            ),
        ));
    }
}
