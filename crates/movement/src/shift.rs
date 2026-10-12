//! World shifts (floating origin, a new planet's frame): engine_world moves the shared
//! contracts (transforms, character state, move intents); this moves movement's own
//! world-space state the same way, in `WorldShiftSet::Apply`.

use engine_core::math::DVec3;
use engine_core::prelude::*;
use engine_world::WorldShift;

use crate::demo::{Brain, DuelDirector};
use crate::mover::{Mover, WallContact};

fn wall(s: &WorldShift, w: WallContact) -> WallContact {
    WallContact {
        normal: s.direction(w.normal),
        point: s.point(w.point),
        ..w
    }
}

pub(crate) fn follow_world_shifts(
    mut shifts: MessageReader<WorldShift>,
    mut movers: Query<(&mut Mover, Option<&GlobalTransform>)>,
    mut brains: Query<&mut Brain>,
    mut directors: Query<&mut DuelDirector>,
) {
    let Some(s) = shifts.read().copied().reduce(|acc, next| acc.then(&next)) else {
        return;
    };
    for (mut m, g) in &mut movers {
        // GlobalTransform still holds the old frame until transforms propagate.
        let p = s.point(g.map_or(DVec3::ZERO, |g| g.translation()));
        m.velocity = s.velocity_at(p, m.velocity);
        m.facing = s.direction(m.facing);
        m.ground_normal = s.direction(m.ground_normal);
        m.up = s.direction(m.up);
        m.knockback = s.direction(m.knockback);
        m.wall = m.wall.map(|w| wall(&s, w));
        m.last_wall = m.last_wall.map(|w| wall(&s, w));
        if let Some(d) = &mut m.dash {
            d.dir = s.direction(d.dir);
        }
        if let Some(mantle) = &mut m.mantle {
            mantle.top = s.point(mantle.top);
            mantle.forward = s.direction(mantle.forward);
        }
        if let Some(req) = &mut m.pending_dash {
            req.to = s.point(req.to);
        }
    }
    for mut b in &mut brains {
        if let Brain::Runner { origin, .. } = &mut *b {
            *origin = s.point(*origin);
        }
    }
    for mut d in &mut directors {
        d.origin = s.point(d.origin);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mover::Mantle;
    use engine_core::math::{DQuat, Vec3};

    #[test]
    fn a_shift_moves_walls_ledges_and_velocities() {
        let mut app = App::new();
        app.add_message::<WorldShift>()
            .add_systems(Update, follow_world_shifts);
        let e = app
            .world
            .spawn((
                GlobalTransform::from(Transform::from_xyz(10.0, 0.0, 0.0)),
                Mover {
                    velocity: Vec3::X * 5.0,
                    facing: Vec3::X,
                    wall: Some(WallContact {
                        normal: Vec3::Z,
                        point: DVec3::new(10.0, 1.0, -1.0),
                        entity: None,
                    }),
                    mantle: Some(Mantle {
                        top: DVec3::new(12.0, 2.0, 0.0),
                        forward: Vec3::X,
                        rising: true,
                        exit_speed: 3.0,
                    }),
                    ..Default::default()
                },
            ))
            .id();
        let shift = WorldShift {
            rotation: DQuat::from_rotation_y(std::f64::consts::FRAC_PI_2),
            translation: DVec3::new(0.0, 0.0, 100.0),
            velocity: DVec3::new(0.0, 0.0, 1.0),
            angular_velocity: DVec3::ZERO,
        };
        app.world.write_message(shift);
        app.update();
        let m = app.world.get::<Mover>(e).unwrap();
        let close = |a: DVec3, b: DVec3| a.distance(b) < 1e-6;
        assert!(close(
            m.wall.unwrap().point,
            shift.point(DVec3::new(10.0, 1.0, -1.0))
        ));
        assert!(m.wall.unwrap().normal.distance(Vec3::X) < 1e-5);
        assert!(close(
            m.mantle.unwrap().top,
            shift.point(DVec3::new(12.0, 2.0, 0.0))
        ));
        assert!(m.facing.distance(Vec3::NEG_Z) < 1e-5);
        // Rotated velocity plus the old frame's own motion.
        assert!(
            m.velocity.distance(Vec3::new(0.0, 0.0, -4.0)) < 1e-4,
            "{}",
            m.velocity
        );
    }
}
