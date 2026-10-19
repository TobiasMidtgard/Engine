//! Auto-exposure after a camera cut: the camera hops from a bright wall to a dim one a few
//! meters along and its exposure jumps to the new view at once, instead of adapting from the
//! old one at the camera's (here very slow) adaptation speed. Needs a GPU or lavapipe; passes
//! with a note when neither is available.

use engine_core::prelude::*;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, RenderStats};

/// Where the dim wall stands, beside the bright one.
const HOP: DVec3 = DVec3::new(15.0, 0.0, 0.0);

fn auto(speed: f32) -> Exposure {
    Exposure::Auto {
        min_ev100: -3.0,
        max_ev100: 18.0,
        speed,
        compensation: 0.0,
    }
}

/// A bright glowing wall at the origin and a dim one at [`HOP`], each filling the view of a
/// camera standing in front of it. Night: nothing else lights the scene.
fn walls_app(camera_at: DVec3) -> App {
    engine_core::log::init("error");
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = 320;
        w.height = 180;
    }
    app.add_plugins(RenderPlugin);
    let w = &mut app.world;
    w.spawn((
        Camera {
            exposure: auto(50.0),
            ..Camera::perspective(60.0)
        },
        Transform::from_translation(camera_at + DVec3::new(0.0, 1.5, 3.0))
            .looking_at(camera_at + DVec3::new(0.0, 1.5, -1.0), Vec3::Y),
    ));
    for (at, nits) in [(DVec3::ZERO, 2000.0), (HOP, 15.0)] {
        w.spawn((
            Transform::from_translation(at + DVec3::new(0.0, 1.5, -1.0)),
            MeshRenderer::new(
                Shape::Cuboid {
                    size: [14.0, 8.0, 0.2],
                },
                StandardMaterial::color(0.0, 0.0, 0.0).with_emissive([1.0, 1.0, 1.0], nits),
            ),
        ));
    }
    app
}

fn camera(app: &mut App) -> Entity {
    app.world
        .query_filtered::<Entity, With<Camera>>()
        .single(&app.world)
        .unwrap()
}

#[test]
fn exposure_jumps_to_the_new_view_after_a_cut() {
    let mut cut = walls_app(DVec3::ZERO);
    cut.update_n(16);
    if let Some(d) = cut.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return;
    }
    let bright = cut.world.resource::<RenderStats>().ev100;

    // Adapt very slowly from now on, then hop to the dim wall.
    let cam = camera(&mut cut);
    cut.world.get_mut::<Camera>(cam).unwrap().exposure = auto(0.001);
    cut.world.get_mut::<Transform>(cam).unwrap().translation += HOP;
    cut.update_n(8);
    let after_cut = cut.world.resource::<RenderStats>().ev100;

    // The dim wall, metered from the start.
    let mut dim = walls_app(HOP);
    dim.update_n(16);
    let dim_ev = dim.world.resource::<RenderStats>().ev100;

    eprintln!("EV100: bright wall {bright}, after the cut {after_cut}, dim wall {dim_ev}");
    assert!(
        bright > dim_ev + 4.0,
        "the walls should meter far apart: {bright} vs {dim_ev}"
    );
    assert!(
        (after_cut - dim_ev).abs() < 0.25,
        "exposure after the cut is {after_cut}, the dim wall meters {dim_ev} (bright wall {bright})"
    );
}

#[test]
fn exposure_still_adapts_gradually_while_the_camera_walks() {
    let mut walk = walls_app(DVec3::ZERO);
    walk.update_n(16);
    if walk.world.get_resource::<RenderDisabled>().is_some() {
        return;
    }
    let bright = walk.world.resource::<RenderStats>().ev100;
    let cam = camera(&mut walk);
    walk.world.get_mut::<Camera>(cam).unwrap().exposure = auto(0.001);
    // Walk to the dim wall in small steps: no cut, so the slow camera barely adapts.
    for _ in 0..60 {
        walk.world.get_mut::<Transform>(cam).unwrap().translation += HOP / 60.0;
        walk.update();
    }
    let walked = walk.world.resource::<RenderStats>().ev100;
    eprintln!("EV100: bright wall {bright}, after walking to the dim wall {walked}");
    assert!(
        (walked - bright).abs() < 1.0,
        "exposure jumped while walking: {bright} -> {walked}"
    );
}
