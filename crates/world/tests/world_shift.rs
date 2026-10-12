//! A `WorldShift` moves everything the shared contracts hold into the new frame in the same
//! frame, and modules see it in `WorldShiftSet::Apply`.

use bevy_ecs::message::{MessageCursor, Messages};
use engine_core::math::{DQuat, DVec2, DVec3, EulerRot, Quat, Vec3};
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_world::locomotion::{LocomotionState, MoveIntent};
use engine_world::physics::{CharacterState, ExternalForce, Velocity};
use engine_world::*;

fn app() -> App {
    let mut app = App::new();
    app.add_plugins(WorldPlugin);
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 60.0,
        });
    app
}

fn turned() -> WorldShift {
    WorldShift {
        rotation: DQuat::from_euler(EulerRot::YXZ, 0.9, 0.25, -0.1),
        translation: DVec3::new(-4.0e5, 120.0, 2.5e4),
        velocity: DVec3::new(0.0, 0.0, 300.0),
        angular_velocity: DVec3::new(0.0, 7.0e-5, 0.0),
    }
}

/// What a module with its own world-space cache does: shift it in `Apply`.
#[derive(Resource, Default)]
struct ModuleCache {
    point: DVec3,
    seen: usize,
}

fn shift_module_cache(mut shifts: MessageReader<WorldShift>, mut cache: ResMut<ModuleCache>) {
    for s in shifts.read() {
        cache.point = s.point(cache.point);
        cache.seen += 1;
    }
}

#[test]
fn entities_and_their_motion_move_into_the_new_frame() {
    let mut app = app();
    app.init_resource::<ModuleCache>()
        .add_systems(PostUpdate, shift_module_cache.in_set(WorldShiftSet::Apply));
    app.world.resource_mut::<ModuleCache>().point = DVec3::new(3.0, 4.0, 5.0);
    let start = Transform::from_xyz(10.0, 2.0, -7.0).with_rotation(Quat::from_rotation_y(0.4));
    let body = app
        .world
        .spawn((
            start,
            TransformInterpolation::default(),
            Velocity {
                linear: Vec3::new(1.0, 0.0, 0.0),
                angular: Vec3::new(0.0, 2.0, 0.0),
            },
            ExternalForce {
                force: Vec3::new(0.0, 50.0, 0.0),
                at: Some(DVec3::new(10.0, 3.0, -7.0)),
                ..Default::default()
            },
            CharacterState {
                ground_normal: Vec3::Y,
                velocity: Vec3::new(0.0, 0.0, -3.0),
                ..Default::default()
            },
            MoveIntent {
                direction: Vec3::X,
                dash_to: Some(DVec3::new(20.0, 0.0, 0.0)),
                ..Default::default()
            },
            LocomotionState {
                velocity: Vec3::new(0.0, 0.0, -3.0),
                ..Default::default()
            },
        ))
        .id();
    let child = app
        .world
        .spawn((Transform::from_xyz(0.0, 1.0, 0.0), ChildOf(body)))
        .id();
    app.update();
    let child_before = app
        .world
        .get::<GlobalTransform>(child)
        .unwrap()
        .translation();
    let gravity_before = app.world.resource::<Gravity>().uniform;

    let s = turned();
    app.world.write_message(s);
    app.update();

    let t = *app.world.get::<Transform>(body).unwrap();
    let p = s.point(start.translation);
    assert!((t.translation - p).length() < 1e-6, "{}", t.translation);
    assert!(t.rotation.angle_between(s.orientation(start.rotation)) < 1e-5);
    // Children follow their parent; their transforms are relative and don't change.
    let child_after = app
        .world
        .get::<GlobalTransform>(child)
        .unwrap()
        .translation();
    assert!(
        (child_after - s.point(child_before)).length() < 1e-4,
        "{child_after}"
    );
    assert_eq!(
        app.world.get::<Transform>(child).unwrap().translation,
        DVec3::new(0.0, 1.0, 0.0)
    );
    // Interpolation starts from the shifted previous state (taken at this frame's fixed step,
    // still in the old frame), so there is no streak.
    let previous = app
        .world
        .get::<TransformInterpolation>(body)
        .unwrap()
        .previous
        .expect("a fixed step ran this frame");
    assert!(
        (previous.translation - p).length() < 1e-6,
        "{}",
        previous.translation
    );
    // Velocities pick up the old frame's own motion.
    let v = app.world.get::<Velocity>(body).unwrap();
    let want = s.velocity_at(p, Vec3::new(1.0, 0.0, 0.0));
    assert!((v.linear - want).length() < 1e-3, "{} vs {want}", v.linear);
    assert!((v.angular - s.spin(Vec3::new(0.0, 2.0, 0.0))).length() < 1e-5);
    let f = app.world.get::<ExternalForce>(body).unwrap();
    assert!((f.force - s.direction(Vec3::new(0.0, 50.0, 0.0))).length() < 1e-3);
    assert!((f.at.unwrap() - s.point(DVec3::new(10.0, 3.0, -7.0))).length() < 1e-6);
    let c = app.world.get::<CharacterState>(body).unwrap();
    assert!((c.ground_normal - s.direction(Vec3::Y)).length() < 1e-5);
    assert!((c.velocity - s.velocity_at(p, Vec3::new(0.0, 0.0, -3.0))).length() < 1e-3);
    let i = app.world.get::<MoveIntent>(body).unwrap();
    assert!((i.direction - s.direction(Vec3::X)).length() < 1e-5);
    assert!((i.dash_to.unwrap() - s.point(DVec3::new(20.0, 0.0, 0.0))).length() < 1e-6);
    // Gravity turns with the world.
    let g = app.world.resource::<Gravity>().uniform;
    assert!((g - s.direction(gravity_before)).length() < 1e-4, "{g}");
    // A module's own cache shifted once, in the same frame.
    let cache = app.world.resource::<ModuleCache>();
    assert_eq!(cache.seen, 1);
    assert!((cache.point - s.point(DVec3::new(3.0, 4.0, 5.0))).length() < 1e-6);

    // Nothing moves again without a new shift.
    app.update_n(3);
    assert!((app.world.get::<Transform>(body).unwrap().translation - p).length() < 1e-6);
    assert_eq!(app.world.resource::<ModuleCache>().seen, 1);
}

#[test]
fn a_floating_origin_move_keeps_ground_state_where_it_was() {
    let mut app = app();
    let spot = DVec3::new(5.5, 0.0, -12.5);
    {
        let mut surface = app.world.resource_mut::<SurfaceLayers>();
        let c = surface.grid.cell_at(spot);
        surface.grid.set(
            c,
            SurfaceCell {
                wetness: 0.8,
                snow_m: 0.3,
                ..Default::default()
            },
        );
        let mut heat = app.world.resource_mut::<HeatField>();
        let c = heat.grid.cell_at(spot);
        heat.grid.set(c, 40.0);
    }
    app.update();
    // Not a whole number of cells, so the grids' origins leave the cell lattice.
    let s = WorldShift::translate(DVec3::new(-1234.4, 50.0, 987.3));
    app.world.write_message(s);
    app.update();
    let moved = s.point(spot);
    let surface = app.world.resource::<SurfaceLayers>();
    assert_eq!(surface.grid.sample_nearest(moved).wetness, 0.8);
    assert_eq!(surface.grid.sample_nearest(moved).snow_m, 0.3);
    assert_eq!(
        app.world.resource::<HeatField>().grid.sample_nearest(moved),
        40.0
    );

    // Recentering later still keeps the values in place (whole cells from the shifted origin).
    let mut surface = app.world.resource_mut::<SurfaceLayers>();
    assert!(
        surface
            .grid
            .recenter(DVec2::new(moved.x + 40.0, moved.z - 30.0))
    );
    assert_eq!(surface.grid.sample_nearest(moved).wetness, 0.8);
}

#[test]
fn a_turned_world_resamples_the_ground_state_around_the_focus() {
    let mut app = app();
    let focus = app
        .world
        .spawn((
            Transform::from_xyz(0.0, 0.0, 0.0),
            SimulationFocus::default(),
        ))
        .id();
    // A wet patch, wider than a cell so the turned cells still land in it.
    let spot = DVec3::new(6.0, 0.0, 3.0);
    {
        let mut surface = app.world.resource_mut::<SurfaceLayers>();
        let c = surface.grid.cell_at(spot);
        for dy in -2..=2 {
            for dx in -2..=2 {
                surface.grid.set(
                    c + engine_core::math::IVec2::new(dx, dy),
                    SurfaceCell {
                        wetness: 1.0,
                        ..Default::default()
                    },
                );
            }
        }
    }
    app.update();
    // A turn about the vertical keeps the ground horizontal, so its state carries over.
    let s = WorldShift {
        rotation: DQuat::from_rotation_y(1.2),
        translation: DVec3::new(300.0, 0.0, -200.0),
        ..WorldShift::IDENTITY
    };
    app.world.write_message(s);
    app.update();
    let moved = s.point(spot);
    let surface = app.world.resource::<SurfaceLayers>();
    assert_eq!(surface.grid.sample_nearest(moved).wetness, 1.0);
    // Centered on the focus in the new frame.
    let focus_now = app
        .world
        .get::<GlobalTransform>(focus)
        .unwrap()
        .translation();
    let half = surface.grid.width as f64 * surface.grid.cell as f64 * 0.5;
    let center = surface.grid.origin + DVec2::splat(half);
    assert!((center - DVec2::new(focus_now.x, focus_now.z)).length() < 1.0);
}

#[test]
fn messages_waiting_to_be_read_move_with_the_world() {
    use engine_world::physics::ApplyImpulse;
    let mut app = app();
    let body = app.world.spawn(Transform::default()).id();
    app.update();
    let s = turned();
    let at = DVec3::new(2.0, 1.0, -3.0);
    // Written in the old frame before the shift, read after it.
    app.world.write_message(ApplyImpulse {
        entity: body,
        impulse: Vec3::new(0.0, 0.0, 10.0),
        at: Some(at),
        angular: Vec3::ZERO,
    });
    app.world.write_message(GroundContact {
        position: at,
        normal: Vec3::Y,
        size: [0.1, 0.3],
        yaw: 0.25,
        load_kg: 80.0,
        speed: 1.0,
        kind: Default::default(),
        surface: None,
        entity: None,
    });
    app.world.write_message(s);
    app.update();
    let impulses: Vec<ApplyImpulse> = MessageCursor::default()
        .read(app.world.resource::<Messages<ApplyImpulse>>())
        .copied()
        .collect();
    assert_eq!(impulses.len(), 1);
    assert!((impulses[0].at.unwrap() - s.point(at)).length() < 1e-6);
    assert!((impulses[0].impulse - s.direction(Vec3::new(0.0, 0.0, 10.0))).length() < 1e-4);
    let contacts: Vec<GroundContact> = MessageCursor::default()
        .read(app.world.resource::<Messages<GroundContact>>())
        .copied()
        .collect();
    assert!((contacts[0].position - s.point(at)).length() < 1e-6);
    assert!((contacts[0].yaw - (0.25 + s.yaw())).abs() < 1e-6);

    // Messages written after the shift are already in the new frame and stay put.
    let fresh = DVec3::new(-7.0, 0.0, 9.0);
    app.world.write_message(NoiseEvent {
        position: fresh,
        radius: 5.0,
        source: None,
    });
    app.update();
    let noises: Vec<NoiseEvent> = MessageCursor::default()
        .read(app.world.resource::<Messages<NoiseEvent>>())
        .copied()
        .collect();
    assert_eq!(noises[0].position, fresh);
}

#[test]
fn the_wind_and_sun_stay_turned() {
    let mut app = app();
    app.update();
    let wind = app.world.resource::<WeatherState>().wind;
    let sun = app.world.resource::<WeatherState>().sun_direction;
    let s = turned();
    app.world.write_message(s);
    app.update_n(5);
    let want = s.direction(wind);
    assert!(
        (app.world.resource::<WindField>().prevailing - want).length() < 1e-4,
        "the wind sync turned the wind back"
    );
    assert!((app.world.resource::<WeatherState>().wind - want).length() < 1e-4);
    assert!(
        (app.world.resource::<WeatherState>().sun_direction - s.direction(sun)).length() < 1e-5
    );
}

#[test]
fn something_spawned_in_the_shift_frame_picks_up_the_right_frame_motion() {
    let mut app = app();
    app.update();
    let start = DVec3::new(4000.0, 10.0, -2500.0);
    let s = turned();
    // Spawned this frame: its `GlobalTransform` is still the origin when the shift applies.
    let e = app
        .world
        .spawn((Transform::from_translation(start), Velocity::default()))
        .id();
    app.world.write_message(s);
    app.update();
    let v = app.world.get::<Velocity>(e).unwrap().linear;
    let want = s.velocity_at(s.point(start), Vec3::ZERO);
    assert!((v - want).length() < 1e-3, "{v} vs {want}");
}

#[test]
fn without_a_focus_a_turned_world_keeps_its_ground_state() {
    let mut app = app();
    let spot = DVec3::new(-20.0, 0.0, 35.0);
    {
        let mut surface = app.world.resource_mut::<SurfaceLayers>();
        // Not square, so both axes have to be centered on their own.
        surface.grid = Grid2::new(
            DVec2::new(-64.0, -128.0),
            2.0,
            64,
            128,
            SurfaceCell::default(),
        );
        let c = surface.grid.cell_at(spot);
        for dy in -2..=2 {
            for dx in -2..=2 {
                surface.grid.set(
                    c + engine_core::math::IVec2::new(dx, dy),
                    SurfaceCell {
                        snow_m: 0.5,
                        ..Default::default()
                    },
                );
            }
        }
    }
    app.update();
    let s = WorldShift {
        rotation: DQuat::from_rotation_y(-0.8),
        translation: DVec3::new(9000.0, 0.0, -4000.0),
        ..WorldShift::IDENTITY
    };
    app.world.write_message(s);
    app.update();
    let surface = app.world.resource::<SurfaceLayers>();
    assert_eq!(surface.grid.sample_nearest(s.point(spot)).snow_m, 0.5);
    assert_eq!((surface.grid.width, surface.grid.height), (64, 128));
}

#[test]
fn gusts_dont_jump_when_the_origin_moves() {
    let mut app = app();
    app.update_n(3);
    let before = app.world.resource::<WindField>().clone();
    let s = WorldShift::translate(DVec3::new(-5000.0, 0.0, 2200.0));
    app.world.write_message(s);
    app.update();
    let wind = app.world.resource::<WindField>();
    // The old field at the new moment (no local disturbances here, so only gust time moved).
    let mut then = before.clone();
    then.time = wind.time;
    for here in [DVec3::new(37.0, 12.0, -81.0), DVec3::new(-3.0, 40.0, 9.0)] {
        let a = wind.sample(s.point(here));
        let b = then.sample(here);
        assert!((a - b).length() < 1e-4, "{a} vs {b} at {here}");
    }
}
