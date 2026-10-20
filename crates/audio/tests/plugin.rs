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
        // Birds and insects would add voices to every test; the nature test turns them on.
        nature: false,
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
fn a_footstep_without_a_speed_goes_at_its_walkers_pace() {
    use engine_world::animation::AnimationParams;
    let mut app = app();
    app.world
        .spawn((AudioListener::default(), Transform::from_xyz(0.0, 1.7, 0.0)));
    app.update_n(3);
    let grass = app.world.resource::<SurfaceMaterials>().id("grass");
    let loudness = |app: &mut App, entity: Entity| {
        app.world.write_message(GroundContact {
            position: DVec3::new(0.5, 0.0, -1.0),
            normal: Vec3::Y,
            size: [0.1, 0.27],
            yaw: 0.0,
            load_kg: 75.0,
            speed: 0.0,
            kind: ContactKind::Foot,
            surface: grass,
            entity: Some(entity),
        });
        app.update();
        let msgs = app.world.resource::<Messages<AudioCue>>();
        msgs.iter_current_update_messages()
            .filter(|c| c.name.starts_with("footstep"))
            .map(|c| c.loudness)
            .next()
            .expect("the step plays")
    };
    // An animation clip's footstep carries no speed; the runner's animation says 4 m/s.
    let mut params = AnimationParams::default();
    params.set("speed", 4.0);
    let runner = app.world.spawn(params).id();
    let standing = app.world.spawn_empty().id();
    let ran = loudness(&mut app, runner);
    let stood = loudness(&mut app, standing);
    assert!(ran > stood * 2.0, "running {ran} vs standing {stood}");
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

#[test]
fn trail_people_ask_for_bodies_and_step_aside_for_them() {
    use engine_world::animation::{BodyRequest, SkeletonPose};
    let mut app = app();
    app.add_systems(Startup, |mut commands: Commands| {
        engine_audio::demo::spawn_demo(&mut commands, DVec3::ZERO);
    });
    #[derive(Resource, Default)]
    struct Steps(Vec<Entity>);
    app.init_resource::<Steps>().add_systems(
        Last,
        |mut contacts: MessageReader<GroundContact>, mut steps: ResMut<Steps>| {
            steps.0.extend(contacts.read().filter_map(|c| c.entity));
        },
    );
    app.update();
    let mut named = |name: &str| {
        app.world
            .query::<(Entity, &Name)>()
            .iter(&app.world)
            .find(|(_, n)| n.as_str() == name)
            .map(|(e, _)| e)
            .unwrap_or_else(|| panic!("no {name}"))
    };
    let (hiker, runner, horse, dog) = (
        named("Trail hiker"),
        named("Trail runner"),
        named("Trail horse"),
        named("Trail dog"),
    );
    for e in [hiker, runner] {
        let body = app
            .world
            .get::<BodyRequest>(e)
            .expect("a person asks for a body");
        assert_eq!(body.kind, "human");
        assert!(body.model.is_some());
    }
    for (e, kind) in [(horse, "horse"), (dog, "dog")] {
        let body = app
            .world
            .get::<BodyRequest>(e)
            .expect("an animal asks for a body");
        assert_eq!(body.kind, kind);
        assert!(body.height.is_some_and(|h| h > 0.3));
    }

    // Until a body arrives the stand-ins walk and step.
    app.update_n(60);
    let steps = std::mem::take(&mut app.world.resource_mut::<Steps>().0);
    assert!(steps.contains(&hiker) && steps.contains(&horse));

    // (Headless, the stand-in's parts have no renderer; give them one to see them hide.)
    let parts: Vec<Entity> = app.world.get::<Children>(hiker).unwrap().to_vec();
    assert!(!parts.is_empty());
    for &p in &parts {
        app.world
            .entity_mut(p)
            .insert(engine_render::MeshRenderer::default());
    }
    let shown = |app: &App| {
        parts
            .iter()
            .filter(|&&p| {
                app.world
                    .get::<engine_render::MeshRenderer>(p)
                    .unwrap()
                    .visible
            })
            .count()
    };

    // The animation module answers for the hiker: its body shows and its own feet step.
    app.world.entity_mut(hiker).insert(SkeletonPose::default());
    app.update_n(60);
    let steps = std::mem::take(&mut app.world.resource_mut::<Steps>().0);
    assert!(!steps.contains(&hiker), "the hiker stepped twice");
    assert!(steps.contains(&horse));
    assert_eq!(shown(&app), 0, "the stand-in still shows");

    // The body goes again: the stand-in is back.
    app.world.entity_mut(hiker).remove::<SkeletonPose>();
    app.update_n(60);
    assert!(app.world.resource::<Steps>().0.contains(&hiker));
    assert_eq!(shown(&app), parts.len());
}

/// A concrete wall: the plane x = 5, as tall and wide as you like.
struct Wall;

impl engine_world::physics::PhysicsQuery for Wall {
    fn raycast(
        &self,
        origin: DVec3,
        dir: Vec3,
        max_distance: f32,
        _: &engine_world::physics::QueryFilter,
    ) -> Option<engine_world::physics::Hit> {
        if dir.x.abs() < 1e-6 {
            return None;
        }
        let t = ((5.0 - origin.x) / dir.x as f64) as f32;
        (t > 0.0 && t <= max_distance).then(|| engine_world::physics::Hit {
            entity: Entity::PLACEHOLDER,
            point: origin + (dir * t).as_dvec3(),
            normal: Vec3::NEG_X,
            distance: t,
            surface: engine_world::SurfaceId(12),
        })
    }
    fn shapecast(
        &self,
        _: &engine_world::physics::ColliderShape,
        _: DVec3,
        _: Quat,
        _: Vec3,
        _: f32,
        _: &engine_world::physics::QueryFilter,
    ) -> Option<engine_world::physics::Hit> {
        None
    }
    fn overlap(
        &self,
        _: &engine_world::physics::ColliderShape,
        _: DVec3,
        _: Quat,
        _: &engine_world::physics::QueryFilter,
        _: &mut Vec<Entity>,
    ) {
    }
    fn closest_point(
        &self,
        _: DVec3,
        _: f32,
        _: &engine_world::physics::QueryFilter,
    ) -> Option<engine_world::physics::Hit> {
        None
    }
}

/// Energy above 2 kHz relative to the whole (how bright a recording is).
fn brightness(samples: &[f32]) -> f32 {
    let mono: Vec<f32> = samples.chunks(2).map(|c| 0.5 * (c[0] + c[1])).collect();
    // First difference ≈ a high-pass: its share of the energy tracks the highs.
    let diff: f32 = mono.windows(2).map(|w| (w[1] - w[0]).powi(2)).sum();
    let all: f32 = mono.iter().map(|x| x * x).sum();
    diff / all.max(1e-12)
}

#[test]
fn walls_muffle_sounds_behind_them() {
    let mut app = app();
    app.insert_resource(engine_world::physics::PhysicsQueries(std::sync::Arc::new(
        Wall,
    )));
    app.world
        .spawn((AudioListener::default(), Transform::from_xyz(0.0, 1.7, 0.0)));
    // Gravel steps 10 m away: first in the open (-x), then behind the wall (+x).
    let open = record(&mut app, "open", 60, |app, i| {
        if i % 15 == 0 {
            step_on(app, "gravel", DVec3::new(-10.0, 0.0, 0.0));
        }
    });
    let walled = record(&mut app, "walled", 60, |app, i| {
        if i % 15 == 0 {
            step_on(app, "gravel", DVec3::new(10.0, 0.0, 0.0));
        }
    });
    let (lo, lw) = (rms(&open), rms(&walled));
    assert!(
        lw < lo * 0.45,
        "the wall should quieten the steps: {lw} vs {lo}"
    );
    let (bo, bw) = (brightness(&open), brightness(&walled));
    assert!(bw < bo * 0.6, "and muffle them: {bw} vs {bo}");

    // Without occlusion the wall changes nothing.
    app.world.resource_mut::<AudioSettings>().occlusion = false;
    let off = record(&mut app, "off", 60, |app, i| {
        if i % 15 == 0 {
            step_on(app, "gravel", DVec3::new(10.0, 0.0, 0.0));
        }
    });
    assert!(rms(&off) > lo * 0.6, "{} vs {lo}", rms(&off));
}

fn heat_disc(app: &mut App, center: DVec3, radius: f64, celsius: f32) {
    let mut heat = app.world.resource_mut::<engine_world::HeatField>();
    let grid = &mut heat.grid;
    let lo = grid.cell_at(center - DVec3::splat(radius));
    let hi = grid.cell_at(center + DVec3::splat(radius));
    for y in lo.y..=hi.y {
        for x in lo.x..=hi.x {
            let c = IVec2::new(x, y);
            let cc = grid.cell_center(c);
            if DVec2::new(cc.x - center.x, cc.y - center.z).length() <= radius {
                grid.set(c, celsius);
            }
        }
    }
}

#[test]
fn burning_ground_and_burning_things_are_heard() {
    let mut app = app();
    app.world
        .spawn((AudioListener::default(), Transform::from_xyz(0.0, 1.7, 0.0)));
    let quiet = record(&mut app, "no_fire", 45, |_, _| {});

    // A patch of grass burning 8 m away.
    heat_disc(&mut app, DVec3::new(8.0, 0.0, 0.0), 3.0, 700.0);
    let mut cues = Vec::new();
    let burning = record(&mut app, "grass_fire", 60, |app, _| {
        let msgs = app.world.resource::<Messages<AudioCue>>();
        cues.extend(
            msgs.iter_current_update_messages()
                .filter(|c| c.name == "fire")
                .filter_map(|c| c.position),
        );
    });
    let quiet_rms = rms(&quiet[quiet.len() / 2..]);
    let fire_rms = rms(&burning[burning.len() / 2..]);
    assert!(
        fire_rms > quiet_rms * 2.0 && fire_rms > 0.01,
        "fire {fire_rms} vs quiet {quiet_rms}"
    );
    assert_eq!(cues.len(), 1, "one patch, one fire: {cues:?}");
    assert!(
        cues[0].distance(DVec3::new(8.0, 0.6, 0.0)) < 1.5,
        "{}",
        cues[0]
    );

    // A burning tree is heard on its own; when the ground goes out its fire fades.
    app.world.spawn((
        Transform::from_xyz(-12.0, 0.0, 0.0),
        engine_world::Combustible {
            fuel_kg: 60.0,
            state: engine_world::BurnState::Burning,
            ..Default::default()
        },
    ));
    heat_disc(&mut app, DVec3::new(8.0, 0.0, 0.0), 3.0, 0.0);
    let before = app.world.resource::<Audio>().voice_count();
    let mut tree_cue = false;
    record(&mut app, "tree_fire", 120, |app, _| {
        let msgs = app.world.resource::<Messages<AudioCue>>();
        tree_cue |= msgs
            .iter_current_update_messages()
            .any(|c| c.name == "fire" && c.position.is_some_and(|p| p.x < -10.0));
    });
    assert!(tree_cue, "the burning tree was never heard");
    assert_eq!(
        app.world.resource::<Audio>().voice_count(),
        before,
        "the grass fire should have faded out as the tree fire started"
    );
}

/// A lake east of x = 6 (its bed sloping down), and a brook running north along x = -9.
struct Lakeside;

impl engine_world::GroundQuery for Lakeside {
    fn sample(&self, p: DVec3) -> Option<engine_world::GroundSample> {
        let height = if p.x > 6.0 {
            -(p.x - 6.0) * 0.3
        } else if (p.x + 9.0).abs() < 1.0 {
            -0.4
        } else {
            0.0
        };
        Some(engine_world::GroundSample {
            height,
            normal: Vec3::Y,
            surface: Default::default(),
        })
    }
}

impl engine_world::WaterQuery for Lakeside {
    fn sample(&self, p: DVec3) -> Option<engine_world::WaterSample> {
        let (depth, velocity) = if p.x > 6.0 {
            ((p.x - 6.0) as f32 * 0.3, Vec3::ZERO)
        } else if (p.x + 9.0).abs() < 1.0 {
            (0.3, Vec3::new(0.0, 0.0, -1.2))
        } else {
            return None;
        };
        Some(engine_world::WaterSample {
            surface_height: if p.x > 6.0 { 0.0 } else { -0.1 },
            depth,
            velocity,
            normal: Vec3::Y,
        })
    }
}

#[test]
fn shores_lap_and_brooks_babble() {
    let mut app = app();
    app.insert_resource(engine_world::Ground(std::sync::Arc::new(Lakeside)));
    app.insert_resource(engine_world::Water(std::sync::Arc::new(Lakeside)));
    let listener = app
        .world
        .spawn((AudioListener::default(), Transform::from_xyz(0.0, 1.7, 0.0)))
        .id();
    let by_water = record(&mut app, "lakeside", 150, |_, _| {});
    // Count only the water: compare with the same place with water sounds off.
    app.world.resource_mut::<AudioSettings>().water = false;
    let dry = record(&mut app, "lakeside_off", 150, |_, _| {});
    let (wet, quiet) = (
        rms(&by_water[by_water.len() / 2..]),
        rms(&dry[dry.len() / 2..]),
    );
    assert!(
        wet > quiet * 1.5 && wet > 0.002,
        "by the water {wet}, off {quiet}"
    );

    // Far inland there's nothing to hear.
    app.world.resource_mut::<AudioSettings>().water = true;
    app.world
        .entity_mut(listener)
        .insert(Transform::from_xyz(-200.0, 1.7, 0.0));
    record(&mut app, "inland_settle", 90, |_, _| {});
    let before = app.world.resource::<Audio>().voice_count();
    app.world.resource_mut::<AudioSettings>().water = false;
    record(&mut app, "inland_off", 90, |_, _| {});
    assert_eq!(app.world.resource::<Audio>().voice_count(), before);
}

#[test]
fn sounds_stay_put_when_the_world_shifts() {
    let mut app = app();
    app.world
        .spawn((AudioListener::default(), Transform::from_xyz(0.0, 1.7, 0.0)));
    app.update();
    {
        let mut audio = app.world.resource_mut::<Audio>();
        let fs = audio.sample_rate();
        audio.play(
            engine_audio::effects::ToneSound::new(fs, 330.0, 4.0, 0.3),
            engine_audio::PlayOptions::at(DVec3::new(5.0, 1.7, 0.0)),
        );
    }
    let x = record(&mut app, "shift", 90, |app, i| {
        if i == 45 {
            // A floating-origin move: everything is now 10 km further along x.
            app.world
                .write_message(engine_world::WorldShift::translate(DVec3::new(
                    10_000.0, 0.0, 0.0,
                )));
        }
    });
    // Stereo: the tone is to the right before and after.
    let window = |a: f32, b: f32| {
        let s = &x[((a * 48_000.0) as usize * 2)..((b * 48_000.0) as usize * 2)];
        let (mut l, mut r) = (0.0f32, 0.0f32);
        for f in s.chunks(2) {
            l += f[0] * f[0];
            r += f[1] * f[1];
        }
        (l.sqrt(), r.sqrt())
    };
    let before = window(0.6, 1.4);
    let after = window(1.7, 2.6);
    let level = |w: (f32, f32)| (w.0 * w.0 + w.1 * w.1).sqrt();
    assert!(
        level(after) > level(before) * 0.8 && level(after) < level(before) * 1.25,
        "level jumped: {before:?} -> {after:?}"
    );
    assert!(after.1 > after.0 * 1.5, "no longer on the right: {after:?}");
}

#[test]
fn lightning_strikes_roll_thunder_late() {
    let mut app = app();
    app.world
        .spawn((AudioListener::default(), Transform::from_xyz(0.0, 1.7, 0.0)));
    let ground = DVec3::new(1_200.0, 0.0, 0.0);
    let mut cues = Vec::new();
    let x = record(&mut app, "lightning", 300, |app, i| {
        if i == 15 {
            app.world.write_message(engine_world::LightningStrike {
                from: ground + DVec3::new(200.0, 2_800.0, 0.0),
                to: ground,
                peak_current_ka: 30.0,
                cloud_to_ground: true,
                seed: 5,
            });
            // The weather's blast at the strike point is part of the thunder, not a bomb.
            app.world.write_message(engine_world::WorldImpulse {
                center: ground,
                radius: 14.0,
                strength: 18.0,
                energy_j: 5.0e5,
                heat_c: 0.0,
                source: None,
            });
        }
        let msgs = app.world.resource::<Messages<AudioCue>>();
        cues.extend(msgs.iter_current_update_messages().map(|c| c.name.clone()));
    });
    assert!(cues.iter().any(|c| c == "thunder"), "{cues:?}");
    assert!(!cues.iter().any(|c| c == "explosion"), "{cues:?}");
    // Silent until the sound has crossed 1.2 km (3.5 s after the flash at 0.5 s), then loud.
    let rms_at = |a: f32, b: f32| rms(&x[(a * 96_000.0) as usize..(b * 96_000.0) as usize]);
    let early = rms_at(0.6, 3.6);
    let rolling = rms_at(4.2, 7.0);
    assert!(
        rolling > 0.02 && rolling > early * 8.0,
        "early {early} rolling {rolling}"
    );
}

#[test]
fn a_waterfall_roars_instead_of_dripping() {
    let mut app = app();
    app.world
        .spawn((AudioListener::default(), Transform::from_xyz(0.0, 1.7, 0.0)));
    let falls = app.world.spawn(Transform::from_xyz(6.0, 0.0, 0.0)).id();
    let mut cues = Vec::new();
    // The water module splashes at the foot of a fall every 0.25 s.
    let x = record(&mut app, "falls", 120, |app, i| {
        if i < 90 && i % 7 == 0 {
            app.world.write_message(engine_world::WaterSplash {
                position: DVec3::new(6.0, 0.0, 0.0),
                velocity: Vec3::new(0.0, -7.0, 0.0),
                radius: 1.0,
                mass_kg: 1.0,
                source: Some(falls),
            });
        }
        let msgs = app.world.resource::<Messages<AudioCue>>();
        cues.extend(msgs.iter_current_update_messages().map(|c| c.name.clone()));
    });
    let sprays = cues.iter().filter(|c| *c == "spray.water").count();
    let drips = cues.iter().filter(|c| *c == "splash.water").count();
    assert_eq!(sprays, 1, "{cues:?}");
    assert!(drips <= 2, "{cues:?}");
    // A steady roar while it falls, gone after it stops.
    let rms_at = |a: f32, b: f32| rms(&x[(a * 96_000.0) as usize..(b * 96_000.0) as usize]);
    let roaring = rms_at(1.5, 2.8);
    assert!(roaring > 0.005, "falls too quiet: {roaring}");
    let quarter = [
        rms_at(1.5, 1.8),
        rms_at(1.8, 2.1),
        rms_at(2.1, 2.4),
        rms_at(2.4, 2.7),
    ];
    let (lo, hi) = quarter
        .iter()
        .fold((f32::MAX, 0.0f32), |(a, b), v| (a.min(*v), b.max(*v)));
    assert!(hi < lo * 2.0, "pulsing, not roaring: {quarter:?}");
}

#[test]
fn engines_rev_and_tyres_skid() {
    use engine_world::{EngineKind, EngineState, WheelContact, WheelContacts};
    let mut app = app();
    app.world
        .spawn((AudioListener::default(), Transform::from_xyz(0.0, 1.7, 0.0)));
    let quiet = record(&mut app, "no_car", 30, |_, _| {});
    let baseline = app.world.resource::<Audio>().voice_count();

    // A V8 car 10 m away cruising on concrete, then locking its wheels.
    let concrete = app
        .world
        .resource::<SurfaceMaterials>()
        .id_or_default("concrete");
    let wheels = |speed: f32, slip: f32| WheelContacts {
        wheels: (0..4)
            .map(|k| WheelContact {
                position: DVec3::new(10.0 + (k % 2) as f64 * 1.6, 0.0, (k / 2) as f64 * 2.6),
                surface: concrete,
                rolling_speed: speed,
                slip,
                load_kg: 400.0,
                radius_m: 0.33,
            })
            .collect(),
    };
    let engine = EngineState {
        kind: EngineKind::Piston { cylinders: 8 },
        running: true,
        rpm: 3_000.0,
        idle_rpm: 800.0,
        max_rpm: 7_000.0,
        throttle: 0.6,
        load: 0.6,
        ..Default::default()
    };
    let car = app
        .world
        .spawn((
            Transform::from_xyz(10.8, 0.4, 1.3),
            engine,
            wheels(15.0, 0.0),
        ))
        .id();
    let mut cues = Vec::new();
    let driving = record(&mut app, "car", 60, |app, i| {
        if i == 30 {
            app.world.entity_mut(car).insert(wheels(15.0, 0.8));
        }
        let msgs = app.world.resource::<Messages<AudioCue>>();
        cues.extend(msgs.iter_current_update_messages().map(|c| c.name.clone()));
    });
    let cruise = rms(&driving[driving.len() / 4..driving.len() / 2]);
    assert!(
        cruise > rms(&quiet) * 3.0 && cruise > 0.005,
        "engine {cruise} vs quiet {}",
        rms(&quiet)
    );
    let count = |name: &str| cues.iter().filter(|c| c.as_str() == name).count();
    assert_eq!(count("engine.piston"), 1, "{cues:?}");
    assert_eq!(count("tyre.skid"), 1, "{cues:?}");
    assert_eq!(app.world.resource::<Audio>().voice_count(), baseline + 2);

    // Parked with the engine off: both fade away.
    app.world.entity_mut(car).insert((
        EngineState {
            running: false,
            rpm: 0.0,
            ..engine
        },
        wheels(0.0, 0.0),
    ));
    record(&mut app, "parked", 60, |_, _| {});
    assert_eq!(app.world.resource::<Audio>().voice_count(), baseline);
}

#[test]
fn sound_travels_at_the_speed_the_air_allows() {
    let mut app = app();
    app.world
        .spawn((AudioListener::default(), Transform::from_xyz(0.0, 1.7, 0.0)));
    let speed = |app: &mut App, celsius: f32| {
        app.world
            .resource_mut::<engine_world::WeatherState>()
            .temperature_c = celsius;
        app.update();
        app.world.resource::<Audio>().speed_of_sound()
    };
    let cold = speed(&mut app, -20.0);
    let mild = speed(&mut app, 15.0);
    let hot = speed(&mut app, 40.0);
    assert!((mild - 343.0).abs() < 3.0, "{mild}");
    assert!(
        cold < mild - 15.0 && hot > mild + 10.0,
        "{cold} {mild} {hot}"
    );
}

#[test]
fn birds_sing_at_dawn_hush_in_rain_and_owls_call_at_night() {
    use engine_world::{PrecipitationKind, WeatherState};
    let mut app = app();
    app.world.resource_mut::<AudioSettings>().nature = true;
    let grass = app
        .world
        .resource::<SurfaceMaterials>()
        .id("grass")
        .expect("grass is built in");
    app.insert_resource(engine_world::Ground(std::sync::Arc::new(Meadow(grass))));
    let listener = app
        .world
        .spawn((AudioListener::default(), Transform::from_xyz(0.0, 1.7, 0.0)))
        .id();
    let set_sky = |app: &mut App, hour: f32, sun_elevation: f32, rain: f32| {
        let mut w = app.world.resource_mut::<WeatherState>();
        w.time_of_day_h = hour;
        w.sun_direction = Vec3::new(1.0, sun_elevation, 0.2).normalize();
        w.precipitation_mm_h = rain;
        w.precipitation_kind = if rain > 0.0 {
            PrecipitationKind::Rain
        } else {
            PrecipitationKind::None
        };
    };
    let birds = |app: &mut App, secs: f32| {
        let mut heard = Vec::new();
        for _ in 0..(secs * 30.0) as usize {
            app.update();
            let msgs = app.world.resource::<Messages<AudioCue>>();
            heard.extend(
                msgs.iter_current_update_messages()
                    .filter(|c| c.name.starts_with("bird."))
                    .map(|c| (c.name.clone(), c.position.unwrap_or_default())),
            );
        }
        heard
    };

    // Just before sunrise: the dawn chorus.
    set_sky(&mut app, 5.5, -0.05, 0.0);
    let dawn = birds(&mut app, 20.0);
    let species: BTreeSet<_> = dawn.iter().map(|(n, _)| n.clone()).collect();
    assert!(dawn.len() >= 6, "dawn chorus: {dawn:?}");
    assert!(species.len() >= 2, "one kind of bird only: {species:?}");
    assert!(!species.contains("bird.owl"), "{species:?}");
    // Up in the trees around the listener, not on top of it.
    for (_, p) in &dawn {
        let d = DVec3::new(p.x, 0.0, p.z).length();
        assert!(d > 10.0 && d < 95.0 && p.y > 2.5, "perched at {p}");
    }

    // Heavy rain: the birds stop (songs already going may finish).
    set_sky(&mut app, 9.0, 0.5, 8.0);
    birds(&mut app, 8.0);
    let rain = birds(&mut app, 15.0);
    assert!(rain.is_empty(), "birds sang in the rain: {rain:?}");

    // A dry night: an owl, and nothing else.
    set_sky(&mut app, 1.0, -0.6, 0.0);
    let night = birds(&mut app, 25.0);
    assert!(
        night.iter().any(|(n, _)| n == "bird.owl"),
        "no owl: {night:?}"
    );
    assert!(night.iter().all(|(n, _)| n == "bird.owl"), "{night:?}");

    // Flying 200 m over the meadow at dawn: the birds are far below.
    set_sky(&mut app, 5.5, -0.05, 0.0);
    app.world
        .entity_mut(listener)
        .insert(Transform::from_xyz(0.0, 200.0, 0.0));
    birds(&mut app, 10.0);
    let aloft = birds(&mut app, 8.0);
    assert!(aloft.is_empty(), "birds sang up in the sky: {aloft:?}");
}

/// Flat grass everywhere.
struct Meadow(engine_world::SurfaceId);

impl engine_world::GroundQuery for Meadow {
    fn sample(&self, _: DVec3) -> Option<engine_world::GroundSample> {
        Some(engine_world::GroundSample {
            height: 0.0,
            normal: Vec3::Y,
            surface: self.0,
        })
    }
}

/// Level of the `hz` tone in the left channel of an interleaved stereo recording.
fn tone_level(samples: &[f32], hz: f32, fs: f32) -> f32 {
    let w = std::f32::consts::TAU * hz / fs;
    let (mut re, mut im) = (0.0f32, 0.0f32);
    for (n, frame) in samples.chunks(2).enumerate() {
        re += frame[0] * (w * n as f32).cos();
        im += frame[0] * (w * n as f32).sin();
    }
    (re * re + im * im).sqrt() / (samples.len() / 2).max(1) as f32
}

#[test]
fn recorded_takes_play_where_there_are_some_and_the_synthesis_everywhere_else() {
    use engine_audio::FootstepSamples;
    use engine_audio::samples::{Gait, SampleBank, Take, TakeSet};
    // A "recording" of concrete that is a 2 kHz tone burst, so it is easy to find.
    let fs = 48_000.0;
    let tone: Vec<f32> = (0..(0.25 * fs) as usize)
        .map(|i| {
            let t = i as f32 / fs;
            (std::f32::consts::TAU * 2_000.0 * t).sin() * (-t * 10.0).exp()
        })
        .collect();
    // Takes come levelled to unit loudness.
    let l = engine_audio::samples::loudness(&tone, fs);
    let tone: Vec<f32> = tone.iter().map(|v| v / l).collect();
    let take = |k: f32| Take {
        pcm: tone.iter().map(|v| v * k).collect::<Vec<_>>().into(),
        sample_rate: fs,
        gait: Gait::Walk,
        source: "tone".into(),
        stats: Default::default(),
    };
    let heard = |surface: &str, recorded: bool| {
        let mut app = app();
        app.world.resource_mut::<AudioSettings>().recorded_footsteps = recorded;
        let mut bank = SampleBank::default();
        bank.surfaces.insert(
            "concrete".into(),
            TakeSet::new(vec![take(1.0), take(0.9), take(1.1)]),
        );
        app.insert_resource(FootstepSamples::from_bank(bank));
        app.world
            .spawn((AudioListener::default(), Transform::from_xyz(0.0, 1.7, 0.0)));
        let s = surface.to_string();
        let x = record(
            &mut app,
            &format!("takes_{surface}_{recorded}"),
            40,
            |app, i| {
                if i % 10 == 0 {
                    step_on(app, &s, DVec3::new(0.3, 0.0, -0.5));
                }
            },
        );
        let fs = app.world.resource::<Audio>().sample_rate();
        (tone_level(&x, 2_000.0, fs), rms(&x))
    };
    let (with, level) = heard("concrete", true);
    let (without, synth_level) = heard("concrete", false);
    assert!(with > without * 2.5, "the take plays: {with} vs {without}");
    // Levelled to the synthesized step, within a few dB.
    let ratio = level / synth_level;
    assert!((0.5..2.0).contains(&ratio), "level ratio {ratio}");
    // Grass has no takes: synthesized, and no tone.
    let (grass, grass_level) = heard("grass", true);
    assert!(
        grass < with * 0.1 && grass_level > 0.001,
        "{grass} {grass_level}"
    );
}
