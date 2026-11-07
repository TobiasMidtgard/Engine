//! Heavy cloth whose frames don't ride the physics step is solved beside the frame, from the
//! end of one fixed step to the start of the next, when the game asks for it: the same steps
//! as at once, a step later, and readers meanwhile see the last step's sim. A body written
//! while its solve is out (cut, grabbed, rebuilt) keeps the write.

use engine_core::diagnostics::Diagnostics;
use engine_core::prelude::*;
use engine_core::time::{FixedTime, TimeSource};
use engine_softbody::{Cloth, ClothPins, SoftBodyConfig, SoftBodyPlugin, SoftBodySim};
use engine_world::WeatherState;

fn app(beside: Option<bool>) -> (App, Entity) {
    let mut app = App::new();
    app.add_plugins((engine_world::WorldPlugin, SoftBodyPlugin));
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 60.0,
        });
    app.world.resource_mut::<FixedTime>().set_hz(60.0);
    app.world.resource_mut::<SoftBodyConfig>().beside_frame = beside;
    let mut weather = app.world.resource_mut::<WeatherState>();
    weather.wind = Vec3::new(3.0, 0.0, 1.0);
    weather.gustiness = 0.0;
    // A big curtain on a still rod: costly enough to leave the frame.
    let rod = app.world.spawn(Transform::from_xyz(0.0, 3.0, 0.0)).id();
    let curtain = app
        .world
        .spawn((
            Transform::IDENTITY,
            Cloth {
                size: [1.5, 2.0],
                resolution: [24, 32],
                pins: ClothPins::TopEdge { every: 2 },
                ..Cloth::default()
            },
            ChildOf(rod),
        ))
        .id();
    (app, curtain)
}

fn positions(app: &App, e: Entity) -> Vec<Vec3> {
    app.world
        .get::<SoftBodySim>(e)
        .expect("built")
        .sim
        .pos
        .clone()
}

fn metric(app: &App, name: &str) -> f64 {
    app.world.resource::<Diagnostics>().get(name).unwrap_or(0.0)
}

#[test]
fn beside_the_frame_a_body_steps_as_at_once_a_step_later() {
    let (mut inline, a) = app(Some(false));
    let (mut beside, b) = app(Some(true));
    inline.update_n(3);
    beside.update_n(4);
    assert!(
        metric(&beside, "softbody.beside_frame_bodies") >= 1.0,
        "the curtain was solved beside the frame"
    );
    assert_eq!(metric(&inline, "softbody.beside_frame_bodies"), 0.0);
    for _ in 0..40 {
        assert_eq!(
            positions(&inline, a),
            positions(&beside, b),
            "beside the frame, each step lands one update later and the same"
        );
        inline.update();
        beside.update();
    }
    // And it moved meanwhile (the wind on a free curtain).
    let (mut still, c) = app(Some(false));
    still.update_n(3);
    assert_ne!(positions(&still, c), positions(&inline, a));
}

#[test]
fn tests_and_headless_runs_step_every_body_at_once() {
    let (mut app, _) = app(None);
    app.update_n(10);
    assert_eq!(metric(&app, "softbody.beside_frame_bodies"), 0.0);
}

#[test]
fn a_body_written_while_its_solve_is_out_keeps_the_write() {
    let (mut app, e) = app(Some(true));
    app.update_n(20);
    // Between two updates its solve is out; move a free particle (as a cut or a grab writes).
    let k = {
        let mut b = app.world.get_mut::<SoftBodySim>(e).unwrap();
        let k = (0..b.sim.len())
            .rfind(|&i| b.sim.inv_mass[i] > 0.0)
            .unwrap();
        b.sim.pos[k].x += 5.0;
        k
    };
    let moved = positions(&app, e)[k];
    app.update();
    assert_eq!(metric(&app, "softbody.beside_frame_dropped"), 1.0);
    // The solve that was out would have put it back; the write stands (the next one, from
    // the written state, lands next update).
    assert_eq!(positions(&app, e)[k], moved);
    app.update_n(5);
    assert_eq!(metric(&app, "softbody.beside_frame_dropped"), 0.0);
    assert!(metric(&app, "softbody.beside_frame_bodies") >= 1.0);
}
