//! Renders the footstep trail offscreen. Needs a GPU or a software Vulkan driver (lavapipe);
//! passes with a note when neither is available.

use engine_audio::{AudioOutput, AudioPlugin, AudioSettings};
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, capture_now};

#[test]
fn the_trail_renders() {
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = 640;
        w.height = 360;
    }
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual { step_secs: 0.05 });
    app.insert_resource(AudioSettings {
        output: AudioOutput::Offline,
        ..Default::default()
    });
    app.add_plugins((RenderPlugin, AudioPlugin));
    app.add_systems(Startup, |mut commands: Commands| {
        commands.spawn((
            DirectionalLight::default(),
            Transform::default().looking_at(DVec3::new(-0.5, -0.6, -0.4), Vec3::Y),
        ));
        commands.spawn((
            Transform::default(),
            MeshRenderer::new(
                Shape::Plane {
                    size: [200.0, 200.0],
                    subdivisions: 1,
                },
                StandardMaterial::hex(0x55743a).with_roughness(0.95),
            ),
        ));
        engine_audio::demo::spawn_demo(&mut commands, DVec3::ZERO);
        commands.spawn((
            Camera::perspective(50.0),
            Transform::from_xyz(0.0, 34.0, 30.0).looking_at(DVec3::ZERO, Vec3::Y),
        ));
    });
    app.update_n(20);
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return;
    }
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("audio_trail.png");
    capture_now(&mut app.world, &path).expect("screenshot");
    let img = image::open(&path).expect("png").to_rgba8();
    let mut distinct = std::collections::HashSet::new();
    let mut bright = 0;
    for (_, _, p) in img.enumerate_pixels() {
        distinct.insert([p[0] / 16, p[1] / 16, p[2] / 16]);
        // Snow, ice and the underpass roof are the only near-white things on the grass.
        if p[0] > 150 && p[1] > 150 && p[2] > 150 {
            bright += 1;
        }
    }
    assert!(
        distinct.len() > 60,
        "image looks flat ({} colors)",
        distinct.len()
    );
    assert!(
        bright > 200,
        "no snow or ice in view ({bright} bright pixels)"
    );
    eprintln!("trail -> {}", path.display());
}
