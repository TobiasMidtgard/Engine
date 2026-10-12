//! Following the world when it re-anchors (a floating-origin move, or a new planet frame; see
//! [`WorldShift`]).
//!
//! The shared contracts move entities' transforms; this module moves what the effects keep of
//! their own: emitter positions and collision planes, trail and beam points, beam targets,
//! and the demo's waypoints. The GPU particles move in the render feature's next simulation
//! step, which takes the shifts collected in [`PendingShift`].

use engine_core::prelude::*;
use engine_world::WorldShift;

use crate::demo;
use crate::emitter::VfxRuntime;
use crate::trail::{Beam, Ribbons};

/// World shifts the GPU particles have not followed yet, combined in order. The render
/// feature takes it when it next simulates.
#[derive(Resource, Debug, Default, Clone, Copy)]
pub struct PendingShift(pub Option<WorldShift>);

impl PendingShift {
    fn push(&mut self, s: WorldShift) {
        self.0 = Some(match self.0 {
            Some(earlier) => earlier.then(&s),
            None => s,
        });
    }
}

/// Runs in `WorldShiftSet::Apply`: moves the effects' own world-space state into the new frame.
pub(crate) fn follow_world_shift(
    mut shifts: MessageReader<WorldShift>,
    mut runtime: ResMut<VfxRuntime>,
    mut ribbons: ResMut<Ribbons>,
    mut pending: ResMut<PendingShift>,
    mut beams: Query<&mut Beam>,
    mut demo: demo::ShiftQueries,
) {
    let Some(s) = shifts.read().copied().reduce(|a, b| a.then(&b)) else {
        return;
    };
    runtime.shift(&s);
    ribbons.shift(&s);
    for mut b in &mut beams {
        b.target = s.point(b.target);
    }
    demo::shift(&mut demo, &s);
    pending.push(s);
}

#[cfg(test)]
mod tests {
    use engine_core::math::DVec3;
    use engine_core::prelude::*;
    use engine_core::time::TimeSource;
    use engine_world::{WorldPlugin, WorldShift};

    use crate::prelude::*;
    use crate::trail::Ribbons;

    #[test]
    fn trails_and_beams_move_with_the_world() {
        let mut app = App::new();
        app.add_plugins((WorldPlugin, VfxPlugin));
        app.world
            .resource_mut::<Time>()
            .set_source(TimeSource::Manual { step_secs: 0.05 });
        let mover = app
            .world
            .spawn((
                Transform::default(),
                Trail {
                    lifetime: 10.0,
                    min_spacing: 0.1,
                    ..Default::default()
                },
            ))
            .id();
        let beam = app
            .world
            .spawn((
                Transform::from_xyz(0.0, 2.0, 0.0),
                Beam {
                    target: DVec3::new(5.0, 0.0, 0.0),
                    ..Default::default()
                },
            ))
            .id();
        for i in 0..10 {
            app.world.get_mut::<Transform>(mover).unwrap().translation =
                DVec3::new(i as f64 * 0.5, 1.0, 0.0);
            app.update();
        }
        let t = DVec3::new(1.0e6, -40.0, 2.5e5);
        app.world.write_message(WorldShift::translate(t));
        app.update();
        // The trail continues where it was (no 1000 km segment back to the old frame).
        let mut gaps = Vec::new();
        app.world
            .resource::<Ribbons>()
            .for_each_ribbon(|points, _, _| {
                for w in points.windows(2) {
                    gaps.push(w[0].pos.distance(w[1].pos));
                }
            });
        assert!(!gaps.is_empty());
        let widest = gaps.iter().copied().fold(0.0, f64::max);
        assert!(widest < 2.0, "trail gap {widest} m after the shift");
        assert_eq!(
            app.world.get::<Beam>(beam).unwrap().target,
            DVec3::new(5.0, 0.0, 0.0) + t
        );
    }
}
