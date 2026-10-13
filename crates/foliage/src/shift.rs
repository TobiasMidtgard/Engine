//! Following a world re-anchoring (`engine_world::WorldShift`): a floating-origin move, or a
//! new planet's frame.
//!
//! `engine_world` moves the root `Transform`s (plants, logs, colliders, glow lights, ground
//! covers, regions). This module moves what foliage keeps in world space of its own, in
//! `WorldShiftSet::Apply`:
//!
//! * each region's center and its simulation (the soil grid and the neighbor buckets). A
//!   translation moves the grids' origins, so every cell keeps its water, nutrients, minerals,
//!   ash and trampling. A rotation resamples the soil into the new frame (each new cell takes
//!   the old cell under it); regions stay rectangles along the new frame's X and Z.
//! * falling trees: the stump, the hinge and the pose they settle from.
//! * which way logs and falling trees lie (`LogPose::yaw`, turned by the shift's yaw).
//! * what plants are posed on ([`PlantBase`]), turned with the world, so a plant's pose keeps
//!   the turn when it grows.
//! * where the glow lights were last gathered.

use engine_core::prelude::*;
use engine_world::WorldShift;

use crate::ecology::RegionState;
use crate::felling::{FallingTree, LogPose};
use crate::plant::Plant;
use crate::visuals::{GlowLights, PlantBase};

/// All shifts written this frame, as one.
fn combined(shifts: &mut MessageReader<WorldShift>) -> Option<WorldShift> {
    shifts.read().fold(None, |acc: Option<WorldShift>, s| {
        Some(acc.map_or(*s, |a| a.then(s)))
    })
}

/// Move foliage's own world-space state into the new frame.
pub fn follow_world_shift(
    mut commands: Commands,
    mut shifts: MessageReader<WorldShift>,
    mut regions: Query<&mut RegionState>,
    mut plants: Query<(Entity, Option<&mut PlantBase>), With<Plant>>,
    mut falling: Query<&mut FallingTree>,
    mut poses: Query<&mut LogPose>,
    mut glow: ResMut<GlowLights>,
) {
    let Some(s) = combined(&mut shifts) else {
        return;
    };
    for mut state in &mut regions {
        let center = state.center;
        state.sim.shift(&s, center);
        state.center = s.point(center);
    }
    for mut f in &mut falling {
        f.stump = s.point(f.stump);
        f.hinge = s.point(f.hinge);
        if let Some((t, from)) = f.settle {
            f.settle = Some((t, s.transform(&from)));
        }
    }
    if !s.is_translation_only() {
        // The root transforms have turned; turn what plants are posed on so their pose
        // keeps that turn when it is next worked out.
        for (e, base) in &mut plants {
            match base {
                Some(mut b) => b.0 = s.orientation(b.0),
                None => {
                    commands
                        .entity(e)
                        .insert(PlantBase(s.orientation(Quat::IDENTITY)));
                }
            }
        }
    }
    let yaw = s.yaw();
    if yaw != 0.0 {
        for mut pose in &mut poses {
            // Yaw is measured from +X toward +Z, the opposite sense to a turn about +Y.
            pose.yaw = (pose.yaw - yaw).rem_euclid(std::f32::consts::TAU);
        }
    }
    glow.follow(&s);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plant::{EcologyRegion, Genes, ResourceSource};
    use crate::species::PlantLibrary;
    use engine_core::math::DQuat;
    use engine_core::time::TimeSource;
    use engine_world::WorldShiftSet;

    fn app() -> App {
        let mut app = App::new();
        app.world
            .resource_mut::<Time>()
            .set_source(TimeSource::Manual {
                step_secs: 1.0 / 60.0,
            });
        app.add_plugins(engine_world::WorldPlugin);
        app.add_plugins(crate::FoliagePlugin);
        app
    }

    fn shift(app: &mut App, s: WorldShift) {
        app.world.resource_mut::<Shifts>().0.push(s);
        app.update();
    }

    #[derive(Resource, Default)]
    struct Shifts(Vec<WorldShift>);

    fn write_shifts(mut pending: ResMut<Shifts>, mut out: MessageWriter<WorldShift>) {
        for s in pending.0.drain(..) {
            out.write(s);
        }
    }

    fn with_shifts(app: &mut App) {
        app.init_resource::<Shifts>()
            .add_systems(PostUpdate, write_shifts.in_set(WorldShiftSet::Write));
    }

    #[test]
    fn soil_and_regions_move_with_the_world() {
        let mut app = app();
        with_shifts(&mut app);
        let region = app
            .world
            .spawn((
                Transform::from_xyz(100.0, 0.0, 50.0),
                EcologyRegion {
                    half_extents: [20.0, 20.0],
                    species: vec!["oak".into()],
                    ..Default::default()
                },
            ))
            .id();
        let lib = PlantLibrary::default();
        app.world.spawn((
            Transform::from_xyz(105.0, 0.0, 55.0),
            Plant::new(lib.id("fern").unwrap(), 0.5, Genes::default()),
        ));
        // A spring just east of the center makes the soil there wetter.
        app.world.spawn((
            Transform::from_xyz(112.0, 0.0, 50.0),
            ResourceSource {
                radius_m: 5.0,
                water: 0.6,
                ..Default::default()
            },
        ));
        app.update_n(3);
        let wet = |app: &App, p: DVec3| {
            let state = app.world.get::<RegionState>(region).unwrap();
            state.sim.soil.sample(p)[0]
        };
        let spring = DVec3::new(112.0, 0.0, 50.0);
        let dry = DVec3::new(88.0, 0.0, 50.0);
        let (w_spring, w_dry) = (wet(&app, spring), wet(&app, dry));
        assert!(w_spring > w_dry + 0.2, "{w_spring} vs {w_dry}");

        // Far from the origin: re-anchor 10 km away. The spring's soil goes with it.
        let s = WorldShift::translate(DVec3::new(-10_000.0, -3.0, 2_500.0));
        shift(&mut app, s);
        let state = app.world.get::<RegionState>(region).unwrap();
        assert_eq!(state.center, s.point(DVec3::new(100.0, 0.0, 50.0)));
        assert_eq!(wet(&app, s.point(spring)), w_spring);
        assert_eq!(wet(&app, s.point(dry)), w_dry);

        // A quarter turn about +Y: the wet side turns with the ground.
        let turn = WorldShift {
            rotation: DQuat::from_rotation_y(std::f64::consts::FRAC_PI_2),
            ..WorldShift::IDENTITY
        };
        let before = (s.point(spring), s.point(dry));
        shift(&mut app, turn);
        let (w2_spring, w2_dry) = (
            wet(&app, turn.point(before.0)),
            wet(&app, turn.point(before.1)),
        );
        assert!(
            (w2_spring - w_spring).abs() < 0.05,
            "{w2_spring} vs {w_spring}"
        );
        assert!((w2_dry - w_dry).abs() < 0.05, "{w2_dry} vs {w_dry}");
        // Plants keep the turn: what they are posed on turned with the world.
        app.update();
        let mut q = app.world.query_filtered::<&PlantBase, With<Plant>>();
        assert!(q.iter(&app.world).count() > 0);
        for b in q.iter(&app.world) {
            assert!(
                b.0.dot(turn.rotation.as_quat()).abs() > 0.99999,
                "{:?} vs {:?}",
                b.0,
                turn.rotation
            );
        }
    }

    #[test]
    fn a_falling_tree_keeps_falling_where_it_stood() {
        let mut app = app();
        with_shifts(&mut app);
        let lib = PlantLibrary::default();
        let tree = app
            .world
            .spawn((
                Transform::from_xyz(3.0, 0.0, 4.0),
                Plant::new(lib.id("birch").unwrap(), 0.8, Genes::default()),
            ))
            .id();
        app.update();
        let log = crate::fell_tree(&mut app.world, tree, Vec3::X).expect("the birch falls");
        app.update_n(5);
        let hinge = app.world.get::<FallingTree>(log).unwrap().hinge;
        let s = WorldShift::translate(DVec3::new(500.0, 0.0, -200.0));
        shift(&mut app, s);
        let f = app.world.get::<FallingTree>(log).unwrap();
        assert_eq!(f.hinge, s.point(hinge));
        // It lands beside its stump in the new frame.
        app.update_n(400);
        let t = app.world.get::<Transform>(log).unwrap();
        assert!(t.translation.distance(s.point(DVec3::new(3.0, 0.0, 4.0))) < 25.0);
    }
}
