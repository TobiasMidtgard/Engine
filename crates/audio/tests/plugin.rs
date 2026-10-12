//! The audio module inside a running app: ground contacts become footsteps you can hear,
//! thin air goes quiet, and the footstep trail walks every surface.

use std::collections::BTreeSet;

use engine_audio::{Audio, AudioCue, AudioListener, AudioOutput, AudioPlugin, AudioSettings};
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_world::{ContactKind, GroundContact, SurfaceMaterials};

fn app() -> App {
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 30.0,
        });
    app.insert_resource(AudioSettings {
        output: AudioOutput::Offline,
        ..Default::default()
    });
    app.add_plugins(AudioPlugin);
    app
}

fn step_on(app: &mut App, surface: &str, at: DVec3) {
    let id = app.world.resource::<SurfaceMaterials>().id(surface);
    app.world.write_message(GroundContact {
        position: at,
        normal: Vec3::Y,
        size: [0.1, 0.27],
        yaw: 0.0,
        load_kg: 75.0,
        speed: 1.4,
        kind: ContactKind::Foot,
        surface: id,
        entity: None,
    });
}

fn rms(samples: &[f32]) -> f32 {
    (samples.iter().map(|s| s * s).sum::<f32>() / samples.len().max(1) as f32).sqrt()
}

fn record(
    app: &mut App,
    name: &str,
    frames: usize,
    mut each: impl FnMut(&mut App, usize),
) -> Vec<f32> {
    let path = std::env::temp_dir().join(format!("engine_audio_{name}_{}.wav", std::process::id()));
    app.update();
    app.world
        .resource_mut::<Audio>()
        .start_recording(&path, 60.0)
        .expect("record");
    for i in 0..frames {
        each(app, i);
        app.update();
    }
    let (path, _) = app
        .world
        .resource_mut::<Audio>()
        .stop_recording()
        .expect("stop");
    let wav = engine_audio::wav::read_wav(&path).expect("read back");
    let _ = std::fs::remove_file(&path);
    wav.samples
}

#[test]
fn footsteps_from_ground_contacts_are_heard() {
    let mut app = app();
    app.world
        .spawn((AudioListener::default(), Transform::from_xyz(0.0, 1.7, 0.0)));
    let samples = record(&mut app, "steps", 45, |app, i| {
        if i % 15 == 0 {
            step_on(app, "mud", DVec3::new(0.3, 0.0, -0.5));
        }
    });
    let audio = app.world.resource::<Audio>();
    assert!(
        audio.is_offline(),
        "headless runs render offline: {}",
        audio.status()
    );
    let level = rms(&samples);
    assert!(
        level > 0.003,
        "three muddy steps should be audible, rms {level}"
    );
    let peak = samples.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    assert!(peak < 1.0, "the limiter keeps it under full scale: {peak}");

    // Silence once they're over.
    let quiet = record(&mut app, "after", 30, |_, _| {});
    assert!(
        rms(&quiet) < level * 0.2,
        "steps end: {} vs {level}",
        rms(&quiet)
    );
}

#[test]
fn footsteps_out_of_earshot_are_skipped() {
    let mut app = app();
    app.world
        .spawn((AudioListener::default(), Transform::from_xyz(0.0, 1.7, 0.0)));
    app.update_n(3);
    // (The wind may already be blowing.)
    let before = app.world.resource::<Audio>().voice_count();
    step_on(&mut app, "gravel", DVec3::new(500.0, 0.0, 0.0));
    app.update();
    assert_eq!(app.world.resource::<Audio>().voice_count(), before);
    step_on(&mut app, "gravel", DVec3::new(5.0, 0.0, 0.0));
    app.update();
    assert_eq!(app.world.resource::<Audio>().voice_count(), before + 1);
}

#[test]
fn there_is_no_sound_in_space() {
    let mut app = app();
    // 120 km up: no air to carry the sound of your own boots on the hull.
    app.world.spawn((
        AudioListener::default(),
        Transform::from_xyz(0.0, 120_000.0, 0.0),
    ));
    let samples = record(&mut app, "space", 30, |app, i| {
        if i % 10 == 0 {
            step_on(app, "metal", DVec3::new(0.2, 119_998.3, 0.0));
        }
    });
    assert!(
        rms(&samples) < 1e-4,
        "space should be silent: {}",
        rms(&samples)
    );
}

#[test]
fn the_trail_walks_every_surface() {
    let mut app = app();
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual { step_secs: 0.1 });
    app.world.spawn((
        AudioListener::default(),
        Transform::from_xyz(0.0, 20.0, 0.0),
    ));
    app.add_systems(Startup, |mut commands: Commands| {
        engine_audio::demo::spawn_demo(&mut commands, DVec3::ZERO);
    });
    #[derive(Resource, Default)]
    struct Heard(BTreeSet<String>);
    app.init_resource::<Heard>().add_systems(
        Last,
        |mut cues: MessageReader<AudioCue>, mut heard: ResMut<Heard>| {
            for c in cues.read() {
                heard.0.insert(c.name.clone());
            }
        },
    );
    // The runner goes round the 116 m loop in under 30 s.
    app.update_n(320);
    let heard = &app.world.resource::<Heard>().0;
    for surface in engine_audio::demo::STRETCHES.iter().map(|s| s.0) {
        let name = format!("footstep.{surface}");
        assert!(heard.contains(&name), "never heard {name}; heard {heard:?}");
    }
}
