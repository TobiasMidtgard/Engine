//! A `WorldShift` moves everything the shared contracts hold into the new frame in the same
//! frame, and modules see it in `WorldShiftSet::Apply`.

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
