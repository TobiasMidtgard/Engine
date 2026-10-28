//! Driving and flying by hand (`engine_vehicles::drive`): E gets in, the keys drive through
//! `ActionState`, the chase camera takes the view, E gets out beside the vehicle once it has
//! stopped. Physics comes from the stand-in solver in `support`.

mod support;

use engine_core::math::{DVec3, Quat, Vec2, Vec3};
use engine_core::prelude::*;
use engine_input::ActionState;
use engine_render::Camera;
use engine_vehicles::drive::{self, ChaseCamera, Driven, Seated};
use engine_vehicles::presets;
use engine_world::physics::Velocity;
use engine_world::{Interact, Interactable};
use support::*;

fn app() -> App {
    let mut app = vehicle_app();
    app.add_plugins(engine_input::InputPlugin);
    calm(&mut app);
    ground(&mut app, DVec3::ZERO, 3_000.0, "concrete");
    app
}

/// A free camera (the actor when nobody walks) at `at`.
fn free_camera(app: &mut App, at: DVec3) -> Entity {
    app.world
        .spawn((Camera::default(), Transform::from_translation(at)))
        .id()
}

fn press_e(app: &mut App, actor: Entity, target: Entity) {
    let from = app
        .world
        .get::<GlobalTransform>(actor)
        .map_or(DVec3::ZERO, |g| g.translation());
    app.world.write_message(Interact {
        actor,
        target,
        from,
    });
    app.update();
}

fn chase_cameras(app: &mut App) -> Vec<(Entity, Camera)> {
    let mut q = app.world.query::<(Entity, &Camera, &ChaseCamera)>();
    q.iter(&app.world).map(|(e, c, _)| (e, *c)).collect()
}

fn speed(app: &App, e: Entity) -> f32 {
    velocity(app, e).linear.length()
}

#[test]
fn walk_up_drive_off_stop_and_get_out() {
    let mut app = app();
    let t = Transform::from_translation(DVec3::Y * presets::ride_height("sports_car"))
        .with_rotation(engine_vehicles::demo::facing(Vec3::NEG_Z));
    let car = spawn_with(&mut app, |c| presets::sports_car(c, t, 0xe0a81c));
    let eye = free_camera(&mut app, DVec3::new(-4.0, 1.7, 0.0));
    run_for(&mut app, 0.5);
    let prompt = app
        .world
        .get::<Interactable>(car)
        .expect("a prompt")
        .clone();
    assert_eq!(prompt.verb, "Drive");
    assert!(prompt.enabled && prompt.reach_m > 3.0, "{prompt:?}");

    press_e(&mut app, eye, car);
    let driven = app.world.get::<Driven>(car).expect("driven").clone();
    assert_eq!(driven.actor, eye);
    assert_eq!(app.world.get::<Seated>(eye).map(|s| s.vehicle), Some(car));
    let cams = chase_cameras(&mut app);
    assert_eq!(cams.len(), 1);
    assert_eq!(cams[0].0, driven.camera);
    assert!(
        cams[0].1.active && cams[0].1.order > 0,
        "drawn over the free camera"
    );

    // W: off it goes, the camera behind it.
    app.world
        .resource_mut::<ActionState>()
        .set_axis("move", Vec2::new(0.0, 1.0), None);
    run_for(&mut app, 3.0);
    let p = transform(&app, car).translation;
    assert!(
        speed(&app, car) > 8.0,
        "{:.1} m/s after 3 s of throttle",
        speed(&app, car)
    );
    assert!(p.z < -10.0, "drove {:.1} m", -p.z);
    let gauges = app.world.resource::<engine_world::Readouts>().lines.clone();
    for label in ["Speed", "Gear"] {
        assert!(
            gauges.iter().any(|(l, _)| l == label),
            "no {label} in {gauges:?}"
        );
    }
    let cam = transform(&app, driven.camera);
    let behind = (cam.translation - p).as_vec3();
    assert!(
        behind.z > 3.0 && behind.y > 0.5,
        "camera at {behind:?} from the car"
    );

    // E while moving: stays in.
    press_e(&mut app, driven.camera, car);
    assert!(app.world.get::<Driven>(car).is_some(), "got out at speed");
    assert_eq!(
        app.world
            .get::<Interactable>(car)
            .map(|i| i.verb.clone())
            .as_deref(),
        Some("Stop to get out")
    );

    // Off the throttle, handbrake on: it stops, and E gets out beside it.
    let mut actions = app.world.resource_mut::<ActionState>();
    actions.release("move");
    actions.press("jump");
    run_until(&mut app, 15.0, |app| speed(app, car) < 0.5);
    assert!(speed(&app, car) < 0.5, "still {:.1} m/s", speed(&app, car));
    app.world.resource_mut::<ActionState>().release("jump");
    press_e(&mut app, driven.camera, car);
    assert!(app.world.get::<Driven>(car).is_none(), "still in");
    assert!(app.world.get::<Seated>(eye).is_none());
    assert!(
        chase_cameras(&mut app).is_empty(),
        "chase camera left behind"
    );
    let car_at = transform(&app, car).translation;
    let out = transform(&app, eye).translation;
    let d = (out - car_at).with_y(0.0).length();
    assert!(d > 1.0 && d < 4.0, "stepped out {d:.1} m from the car");
    assert!((out.y - 1.7).abs() < 0.3, "eye at {:.2} m", out.y);
    assert_eq!(
        app.world
            .get::<Interactable>(car)
            .map(|i| i.verb.clone())
            .as_deref(),
        Some("Drive")
    );
}

/// The camera is the aim: turned up and to the right, the plane climbs into a right turn.
#[test]
fn a_plane_flies_where_the_camera_looks() {
    let mut app = app();
    let t = Transform::from_translation(DVec3::new(0.0, 400.0, 0.0))
        .with_rotation(engine_vehicles::demo::facing(Vec3::NEG_Z));
    let plane = spawn_with(&mut app, |c| presets::sport_plane(c, t));
    app.world.entity_mut(plane).insert((
        Name::new("Hand plane"),
        Velocity {
            linear: Vec3::NEG_Z * 60.0,
            angular: Vec3::ZERO,
        },
    ));
    free_camera(&mut app, DVec3::new(0.0, 400.0, 20.0));
    app.update();
    let r = engine_core::registry::call_api(
        &mut app.world,
        "vehicles.board",
        serde_json::json!({"vehicle": "Hand plane"}),
    );
    assert!(r.is_ok(), "{r:?}");
    let camera = app.world.get::<Driven>(plane).expect("driven").camera;
    app.world.resource_mut::<ActionState>().press("sprint");
    // Look 12° up and 40° right.
    let aim =
        Quat::from_rotation_y(-40f32.to_radians()) * Quat::from_rotation_x(12f32.to_radians());
    app.world
        .get_mut::<Transform>(camera)
        .expect("camera")
        .rotation = aim;
    let start = transform(&app, plane).translation;
    run_for(&mut app, 6.0);
    let p = transform(&app, plane).translation;
    let fwd = transform(&app, plane).forward();
    assert!(p.y > start.y + 15.0, "climbed {:.1} m", p.y - start.y);
    assert!(fwd.x > 0.3, "turned right: nose {fwd:?}");
    assert!(speed(&app, plane) > 35.0, "{:.0} m/s", speed(&app, plane));

    // Out by the API at once, even in flight; the actor stays put with `stay`.
    let r = engine_core::registry::call_api(
        &mut app.world,
        "vehicles.board",
        serde_json::json!({"vehicle": "Hand plane", "out": true, "stay": true}),
    );
    assert_eq!(r, Ok(serde_json::json!({"out": true})));
    app.update();
    assert!(app.world.get::<Driven>(plane).is_none());
    assert!(chase_cameras(&mut app).is_empty());
}

#[test]
fn mouse_aim_banks_into_turns_and_levels_the_wings_on_target() {
    let up = Vec3::Y;
    // Aim up: nose up, no roll.
    let (pitch, yaw, roll) =
        drive::mouse_aim(Quat::IDENTITY, Vec3::new(0.0, 0.2, -1.0), up, true, 2.5);
    assert!(
        pitch > 0.3 && yaw.abs() < 0.01 && roll.abs() < 0.01,
        "{pitch} {yaw} {roll}"
    );
    // Aim far right: yaw and roll right.
    let (_, yaw, roll) = drive::mouse_aim(Quat::IDENTITY, Vec3::new(1.0, 0.0, -1.0), up, true, 2.5);
    assert!(yaw > 0.5 && roll > 0.5, "{yaw} {roll}");
    // On target while banked right: roll left to level.
    let banked = Quat::from_rotation_z(-0.6);
    let (_, _, roll) = drive::mouse_aim(banked, banked * Vec3::NEG_Z, up, true, 2.5);
    assert!(roll < -0.5, "{roll}");
    // Behind: turn hard toward its side.
    let (_, yaw, _) = drive::mouse_aim(Quat::IDENTITY, Vec3::new(-0.1, 0.0, 1.0), up, false, 2.5);
    assert!(yaw <= -1.0 + 1e-6, "{yaw}");
}
