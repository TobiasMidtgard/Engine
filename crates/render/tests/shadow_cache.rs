//! Shadow maps keep a static image of their still casters (`RenderSettings::shadow_cascade_reuse`)
//! and draw only the moving ones on top each frame: a camera walking through a field of pillars
//! and a dense boulder, past a moving crate, sees the same shadows as with every caster drawn each
//! frame, while drawing less. Needs a GPU or lavapipe; passes with a note when neither is
//! available.

use engine_assets::Assets;
use engine_core::prelude::*;
use engine_render::prelude::*;
use engine_render::rock::rock;
use engine_render::{Mesh, RenderDisabled, RenderStats, capture_now};

const W: u32 = 320;
const H: u32 = 180;
const FRAMES: u32 = 12;

/// Walk the camera for a few frames, with static shadow images (`reuse` > 0) or without: the
/// draw calls over the walk and the last frame's pixels.
fn walk(reuse: u32, shot: &str) -> Option<(u32, Vec<u8>)> {
    engine_core::log::init("error");
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = W;
        w.height = H;
    }
    app.add_plugins(RenderPlugin);
    {
        let mut s = app.world.resource_mut::<RenderSettings>();
        s.anti_aliasing = AntiAliasing::None;
        s.film_grain = 0.0;
        s.shadow_cascade_reuse = reuse;
    }
    let w = &mut app.world;
    let camera = w
        .spawn((
            Camera::perspective(60.0),
            Transform::from_xyz(0.0, 2.0, 0.0).looking_at(DVec3::new(0.0, 0.0, -60.0), Vec3::Y),
        ))
        .id();
    // A low sun: long shadows reaching across the far cascades.
    let e = 18f64.to_radians();
    w.spawn((
        DirectionalLight::default(),
        Transform::default()
            .looking_at(DVec3::new(0.7 * e.cos(), -e.sin(), -0.7 * e.cos()), Vec3::Y),
    ));
    w.spawn((
        Transform::default(),
        MeshRenderer::new(
            Shape::Plane {
                size: [800.0, 800.0],
                subdivisions: 1,
            },
            StandardMaterial::color(0.5, 0.5, 0.45),
        ),
    ));
    let pillar = StandardMaterial::color(0.6, 0.55, 0.5);
    for i in 0..48 {
        let z = -6.0 - (i as f64).powf(1.6) * 1.5;
        let x = if i % 2 == 0 { -4.0 } else { 4.0 } * (1.0 + i as f64 * 0.15);
        w.spawn((
            Transform::from_xyz(x, 2.0, z),
            MeshRenderer::new(
                Shape::Cuboid {
                    size: [1.0, 4.0, 1.0],
                },
                pillar,
            ),
        ));
    }
    // Dense enough for the virtual geometry path, far enough for the third and fourth cascades.
    let boulder = w
        .resource_mut::<Assets<Mesh>>()
        .add(rock(Vec3::new(6.0, 4.0, 5.0), 3, 6));
    w.spawn((
        Transform::from_xyz(-10.0, 1.0, -120.0),
        MeshRenderer::new(boulder, StandardMaterial::color(0.4, 0.38, 0.35)),
    ));
    // A crate sliding across the view: its shadow must be right every frame.
    let crate_ = w
        .spawn((
            Transform::from_xyz(-3.0, 0.5, -9.0),
            MeshRenderer::new(
                Shape::Cuboid {
                    size: [1.0, 1.0, 1.0],
                },
                StandardMaterial::color(0.5, 0.3, 0.2),
            ),
        ))
        .id();
    // Settle: the boulder's clusters are built in the background, and the still casters join
    // the static images once they have been still for a while (well under 200 frames).
    let start = std::time::Instant::now();
    let mut frames = 0;
    while start.elapsed().as_secs() < 120 {
        app.update();
        frames += 1;
        if app.world.get_resource::<RenderDisabled>().is_some() {
            break;
        }
        if frames >= 200
            && app
                .world
                .get_resource::<RenderStats>()
                .is_some_and(|s| s.clusters > 0)
        {
            break;
        }
    }
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return None;
    }
    let mut draws = 0;
    for _ in 0..FRAMES {
        app.world
            .get_mut::<Transform>(camera)
            .expect("camera")
            .translation
            .z -= 0.03;
        app.world
            .get_mut::<Transform>(crate_)
            .expect("crate")
            .translation
            .x += 0.25;
        app.update();
        draws += app.world.resource::<RenderStats>().draw_calls;
    }
    let dir = std::env::temp_dir().join("engine-shadow-cache");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join(shot);
    capture_now(&mut app.world, &path).expect("screenshot");
    if let Ok(out) = std::env::var("ENGINE_SHOTS") {
        let _ = std::fs::copy(&path, std::path::Path::new(&out).join(shot));
    }
    let image = image::open(&path).expect("read the shot").to_rgb8();
    Some((draws, image.into_raw()))
}

#[test]
fn static_shadow_images_cast_the_same_shadows_with_fewer_draws() {
    let Some((every, before)) = walk(0, "shadows_every_frame.png") else {
        return;
    };
    let Some((reused, after)) = walk(3, "shadows_static_images.png") else {
        return;
    };
    eprintln!(
        "draw calls over the walk: {every} drawing every caster, {reused} with static images"
    );
    assert!(reused < every, "{reused} draws reusing, {every} without");
    let diff = before
        .iter()
        .zip(&after)
        .map(|(a, b)| (*a as i32 - *b as i32).unsigned_abs())
        .sum::<u32>() as f32
        / before.len() as f32;
    eprintln!("mean difference {diff:.2} of 255");
    assert!(
        diff < 1.0,
        "the images differ by {diff:.2} of 255 on average"
    );
}

/// Swaying casters too far to show their motion in a shadow map rest in its static image in one
/// pose: a camera that flies off (every image drawn again) and comes back a second later sees
/// the same shadows, though the wind's gusts moved on. And casters spawned in place join the
/// static images two frames later without drawing them again (a tree streamed in on a fast fly
/// used to wait 90 frames, then pop in).
#[test]
fn swaying_shadows_rest_in_one_pose_and_new_casters_join_at_once() {
    engine_core::log::init("error");
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = W;
        w.height = H;
    }
    app.add_plugins(RenderPlugin);
    {
        let mut s = app.world.resource_mut::<RenderSettings>();
        s.anti_aliasing = AntiAliasing::None;
        s.film_grain = 0.0;
        s.shadow_cascade_reuse = 3;
    }
    app.world.insert_resource(engine_world::WeatherState {
        wind: Vec3::new(15.0, 0.0, 0.0),
        ..Default::default()
    });
    let w = &mut app.world;
    // High above, looking down on the posts' shadows (the sky and its moving clouds out of
    // view).
    let home =
        Transform::from_xyz(0.0, 40.0, -20.0).looking_at(DVec3::new(0.0, 0.0, -80.0), Vec3::Y);
    let camera = w.spawn((Camera::perspective(60.0), home)).id();
    let e = 30f64.to_radians();
    w.spawn((
        DirectionalLight::default(),
        Transform::default()
            .looking_at(DVec3::new(0.7 * e.cos(), -e.sin(), -0.7 * e.cos()), Vec3::Y),
    ));
    w.spawn((
        Transform::default(),
        MeshRenderer::new(
            Shape::Plane {
                size: [800.0, 800.0],
                subdivisions: 1,
            },
            StandardMaterial::color(0.5, 0.5, 0.45),
        ),
    ));
    // Swaying posts that only cast (so the view shows their shadows alone), past the finest
    // cascade.
    let post = |w: &mut World, x: f64, z: f64| {
        let mut material = StandardMaterial::color(0.3, 0.5, 0.2);
        material.wind_sway = 1.0;
        w.spawn((
            Transform::from_xyz(x, 3.0, z),
            MeshRenderer::new(
                Shape::Cuboid {
                    size: [0.6, 6.0, 0.6],
                },
                material,
            ),
            ShadowOnly,
        ));
    };
    for i in 0..24 {
        let z = -36.0 - i as f64 * 4.0;
        post(w, if i % 2 == 0 { -5.0 } else { 5.0 }, z);
    }
    app.update_n(120);
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return;
    }
    let dir = std::env::temp_dir().join("engine-shadow-cache");
    let _ = std::fs::create_dir_all(&dir);
    let shoot = |app: &mut App, name: &str| {
        let path = dir.join(name);
        capture_now(&mut app.world, &path).expect("screenshot");
        if let Ok(out) = std::env::var("ENGINE_SHOTS") {
            let _ = std::fs::copy(&path, std::path::Path::new(&out).join(name));
        }
        image::open(&path).expect("read the shot").to_rgb8()
    };
    let before = shoot(&mut app, "sway_rest_before.png");
    // Off and back: every static image is drawn again twice, a second of wind apart.
    *app.world.get_mut::<Transform>(camera).expect("camera") =
        Transform::from_xyz(300.0, 40.0, -20.0).looking_at(DVec3::new(300.0, 0.0, -80.0), Vec3::Y);
    app.update_n(60);
    *app.world.get_mut::<Transform>(camera).expect("camera") = home;
    app.update_n(4);
    let after = shoot(&mut app, "sway_rest_after.png");
    // Pixels that went in or out of shadow (a shadow edge moved), not noise.
    let flipped = before
        .pixels()
        .zip(after.pixels())
        .filter(|(a, b)| {
            let l = |p: &image::Rgb<u8>| p.0.iter().map(|&c| c as i32).sum::<i32>();
            (l(a) - l(b)).abs() > 60
        })
        .count();
    let shaded = before
        .pixels()
        .filter(|p| p.0.iter().map(|&c| c as u32).sum::<u32>() < 300)
        .count();
    eprintln!(
        "swaying shadows after the fly off and back: {flipped} pixels flipped of {shaded} shaded"
    );
    assert!(
        shaded > 100,
        "the posts' shadows aren't in view ({shaded} px)"
    );
    assert!(
        flipped * 50 < shaded,
        "{flipped} pixels went in or out of shadow (of {shaded} shaded)"
    );

    // New posts, spawned in place while the camera holds still.
    let diag = |app: &App, key: &str| {
        app.world
            .resource::<Diagnostics>()
            .get(&format!("render.meshes.{key}"))
            .unwrap_or(-1.0)
    };
    for i in 0..10 {
        post(
            &mut app.world,
            -12.0 + i as f64 * 2.5,
            -70.0 - i as f64 * 6.0,
        );
    }
    let (mut joined, mut redrawn) = (0.0, 0.0);
    for _ in 0..5 {
        app.update();
        joined += diag(&app, "shadow_joined");
        redrawn += diag(&app, "static_redrawn");
    }
    eprintln!("new posts: {joined} joins, {redrawn} static images drawn again");
    assert!(joined >= 10.0, "{joined} joins for 10 new posts");
    assert_eq!(
        redrawn, 0.0,
        "a static image was drawn again for new still casters"
    );
}
