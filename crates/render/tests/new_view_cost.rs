//! A new extra view (a mirror coming into range, a portal, an inset) builds its targets but
//! compiles no shader or pipeline: it takes the main view's from the renderer's cache
//! (`gpu_cache`). On Tobias's PC the lake mirror's first frame compiled them all again (about
//! 90 ms on the main thread); under lavapipe about 850 ms. And the views of a frame share its
//! number, so the main view steps through every TAA jitter offset and keeps its intervals with
//! a mirror in the scene. Needs a GPU or lavapipe; passes with a note when neither is
//! available.

use engine_core::prelude::*;
use engine_render::prelude::*;
use engine_render::{ExtraView, RenderDisabled, with_renderer};

#[test]
fn a_new_extra_view_compiles_nothing() {
    engine_core::log::init("error");
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = 160;
        w.height = 90;
    }
    app.add_plugins(RenderPlugin);
    app.world.spawn((
        Camera::perspective(60.0),
        Transform::from_xyz(0.0, 2.0, 6.0).looking_at(DVec3::ZERO, Vec3::Y),
    ));
    app.world.spawn((
        DirectionalLight::default(),
        Transform::default().looking_at(DVec3::new(0.3, -1.0, -0.2), Vec3::Y),
    ));
    for _ in 0..3 {
        app.update();
    }
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("no GPU ({}), skipping", d.0);
        return;
    }
    // A view of another size, appearing, going and coming back (each time a new state).
    for round in 0..2 {
        let view = app
            .world
            .spawn((
                Camera {
                    order: 1,
                    ..Camera::perspective(50.0)
                },
                ExtraView::window(),
                Viewport::new(UVec2::new(80, 45), UVec2::new(80, 45)),
                Transform::from_xyz(0.0, 3.0, 6.0).looking_at(DVec3::ZERO, Vec3::Y),
            ))
            .id();
        app.update();
        let d = app.world.resource::<Diagnostics>();
        let compiled = d.metrics["render.extra_views_new_compiled"];
        let ms = d.metrics["render.extra_views_new_ms"];
        eprintln!("round {round}: new view state in {ms:.1} ms, {compiled} compiled");
        assert_eq!(
            compiled, 0.0,
            "round {round}: the new view compiled {compiled}"
        );
        app.world.despawn(view);
        app.update();
    }
}

/// An extra view doesn't advance the frame count: with one the main view saw only every other
/// frame number, so only half of TAA's jitter offsets (and of the screen effects' noise
/// patterns), and its environment map was drawn twice as often as its interval says.
#[test]
fn extra_views_share_the_frame_number() {
    engine_core::log::init("error");
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = 160;
        w.height = 90;
    }
    app.add_plugins(RenderPlugin);
    app.world.spawn((
        Camera::perspective(60.0),
        Transform::from_xyz(0.0, 2.0, 6.0).looking_at(DVec3::ZERO, Vec3::Y),
    ));
    app.world.spawn((
        Camera {
            order: 1,
            ..Camera::perspective(50.0)
        },
        ExtraView::window(),
        Viewport::new(UVec2::new(80, 45), UVec2::new(80, 45)),
        Transform::from_xyz(0.0, 3.0, 6.0).looking_at(DVec3::ZERO, Vec3::Y),
    ));
    app.update();
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("no GPU ({}), skipping", d.0);
        return;
    }
    let frame = |app: &mut App| {
        with_renderer(&mut app.world, |r, _| r.describe()["frame"].as_u64())
            .flatten()
            .expect("the renderer reports its frame")
    };
    let before = frame(&mut app);
    for _ in 0..10 {
        app.update();
    }
    let after = frame(&mut app);
    assert_eq!(
        after - before,
        10,
        "10 frames with an extra view counted as {}",
        after - before
    );
}
