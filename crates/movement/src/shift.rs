//! World shifts (floating origin, a new planet's frame): engine_world moves the shared
//! contracts (transforms, character state, move intents); this moves movement's own
//! world-space state the same way, in `WorldShiftSet::Apply`.

use engine_core::math::DVec3;
use engine_core::prelude::*;
use engine_world::WorldShift;

use crate::afterimage::{Afterimage, Afterimages};
use crate::demo::{Brain, DuelDirector, Shuttle};
use crate::look::effects::Effects;
use crate::mover::{Mover, WallContact};

fn wall(s: &WorldShift, w: WallContact) -> WallContact {
    WallContact {
        normal: s.direction(w.normal),
        point: s.point(w.point),
        ..w
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn follow_world_shifts(
    mut shifts: MessageReader<WorldShift>,
    mut movers: Query<(&mut Mover, Option<&GlobalTransform>)>,
    mut brains: Query<&mut Brain>,
    mut directors: Query<&mut DuelDirector>,
    mut shuttles: Query<&mut Shuttle>,
    mut trails: Query<&mut Afterimages>,
    mut ghosts: Query<&mut Afterimage>,
    effects: Option<ResMut<Effects>>,
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
        if let Some(hang) = &mut m.hang {
            hang.top = s.point(hang.top);
            hang.normal = s.direction(hang.normal);
            if let Some(c) = &mut hang.corner {
                for p in [
                    &mut c.from,
                    &mut c.via,
                    &mut c.to,
                    &mut c.from_top,
                    &mut c.top,
                ] {
                    *p = s.point(*p);
                }
                for d in [&mut c.from_normal, &mut c.normal, &mut c.along, &mut c.came] {
                    *d = s.direction(*d);
                }
            }
            hang.carry = hang.carry.map(|(a, b)| (s.direction(a), s.direction(b)));
        }
        if let Some(req) = &mut m.pending_dash {
            req.to = s.point(req.to);
        }
        if let Some(g) = &mut m.grapple {
            g.anchor = s.point(g.anchor);
            g.normal = s.direction(g.normal);
            g.anchor_velocity = s.velocity_at(g.anchor, g.anchor_velocity);
        }
        m.pending_grapple = m.pending_grapple.map(|at| s.point(at));
        if let Some(z) = &mut m.zip {
            z.heading = s.direction(z.heading);
        }
        if let Some(l) = &mut m.ladder {
            l.facing = s.direction(l.facing);
        }
        if let Some(platform) = &mut m.platform {
            platform.shift(&s);
        }
    }
    for mut b in &mut brains {
        if let Brain::Runner { origin, .. }
        | Brain::Climber { origin, .. }
        | Brain::LadderClimber { origin, .. }
        | Brain::Grappler { origin, .. }
        | Brain::Rider { origin, .. } = &mut *b
        {
            *origin = s.point(*origin);
        }
    }
    for mut d in &mut directors {
        d.origin = s.point(d.origin);
    }
    for mut sh in &mut shuttles {
        sh.from = s.point(sh.from);
        sh.to = s.point(sh.to);
    }
    for mut a in &mut trails {
        a.shift(&s);
    }
    for mut g in &mut ghosts {
        g.shift(&s);
    }
    if let Some(mut fx) = effects {
        fx.shift(&s);
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
                        on: None,
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

    #[test]
    fn a_shift_moves_a_hang_rounding_a_corner() {
        use crate::mover::{Corner, Hang};
        let mut app = App::new();
        app.add_message::<WorldShift>()
            .add_systems(Update, follow_world_shifts);
        let corner = Corner {
            from: DVec3::new(1.0, 2.0, 0.3),
            via: DVec3::new(1.3, 2.0, 0.3),
            to: DVec3::new(1.3, 2.0, 0.0),
            from_top: DVec3::new(1.0, 3.0, 0.0),
            from_normal: Vec3::Z,
            top: DVec3::new(1.0, 3.0, -0.1),
            normal: Vec3::X,
            along: Vec3::NEG_Z,
            came: Vec3::X,
            wall: None,
            t: 0.5,
            secs: 0.4,
        };
        let mut hang = Hang::new(DVec3::new(1.0, 3.0, 0.0), Vec3::Z, None);
        hang.corner = Some(corner);
        hang.carry = Some((Vec3::X, Vec3::NEG_Z));
        let e = app
            .world
            .spawn((
                GlobalTransform::default(),
                Mover {
                    hang: Some(hang),
                    ..Default::default()
                },
            ))
            .id();
        let shift = WorldShift {
            rotation: DQuat::from_rotation_y(std::f64::consts::FRAC_PI_2),
            translation: DVec3::new(0.0, 0.0, 100.0),
            velocity: DVec3::ZERO,
            angular_velocity: DVec3::ZERO,
        };
        app.world.write_message(shift);
        app.update();
        let h = app.world.get::<Mover>(e).unwrap().hang.unwrap();
        let c = h.corner.unwrap();
        let close = |a: DVec3, b: DVec3| a.distance(b) < 1e-6;
        for (now, was) in [
            (c.from, corner.from),
            (c.via, corner.via),
            (c.to, corner.to),
            (c.from_top, corner.from_top),
            (c.top, corner.top),
        ] {
            assert!(close(now, shift.point(was)), "{now} vs {was}");
        }
        assert!(c.normal.distance(shift.direction(Vec3::X)) < 1e-5);
        assert!(c.along.distance(shift.direction(Vec3::NEG_Z)) < 1e-5);
        assert!(c.came.distance(shift.direction(Vec3::X)) < 1e-5);
        assert!(c.from_normal.distance(shift.direction(Vec3::Z)) < 1e-5);
        let (came, along) = h.carry.unwrap();
        assert!(came.distance(c.came) < 1e-5 && along.distance(c.along) < 1e-5);
        assert_eq!((c.t, c.secs), (0.5, 0.4));
    }

    #[test]
    fn a_shift_turns_ladder_climbers_and_zip_line_riders() {
        use crate::demo::Rungs;
        use crate::mover::{Ladder, Zip};
        let mut app = App::new();
        app.add_message::<WorldShift>()
            .add_systems(Update, follow_world_shifts);
        let climber = app
            .world
            .spawn((
                GlobalTransform::default(),
                Mover {
                    ladder: Some(Ladder {
                        entity: None,
                        t: 0.5,
                        climbed: 2.0,
                        speed: 2.4,
                        facing: Vec3::X,
                        sliding: false,
                        secs: 1.0,
                        rung: 6,
                    }),
                    ..Default::default()
                },
                Brain::LadderClimber {
                    step: Rungs::Up,
                    secs: 0.0,
                    origin: DVec3::new(1.0, 0.0, 0.0),
                },
            ))
            .id();
        let rider = app
            .world
            .spawn((
                GlobalTransform::default(),
                Mover {
                    zip: Some(Zip {
                        line: None,
                        s: 0.4,
                        dir: 1.0,
                        speed: 12.0,
                        heading: Vec3::new(0.6, -0.8, 0.0),
                        secs: 1.0,
                    }),
                    ..Default::default()
                },
            ))
            .id();
        let shift = WorldShift {
            rotation: DQuat::from_rotation_y(std::f64::consts::FRAC_PI_2),
            translation: DVec3::new(0.0, 0.0, 100.0),
            velocity: DVec3::ZERO,
            angular_velocity: DVec3::ZERO,
        };
        app.world.write_message(shift);
        app.update();
        let m = app.world.get::<Mover>(climber).unwrap();
        assert!(m.ladder.unwrap().facing.distance(shift.direction(Vec3::X)) < 1e-5);
        let Brain::LadderClimber { origin, .. } = app.world.get::<Brain>(climber).unwrap() else {
            unreachable!()
        };
        assert!(origin.distance(shift.point(DVec3::new(1.0, 0.0, 0.0))) < 1e-6);
        let heading = app.world.get::<Mover>(rider).unwrap().zip.unwrap().heading;
        assert!(heading.distance(shift.direction(Vec3::new(0.6, -0.8, 0.0))) < 1e-5);
    }

    #[test]
    fn sparks_in_flight_move_with_the_world() {
        use crate::look::effects::EffectSettings;
        let mut app = App::new();
        app.add_message::<WorldShift>()
            .add_systems(Update, follow_world_shifts);
        let mut fx = Effects::default();
        fx.parry(
            DVec3::new(5.0, 1.5, 0.0),
            Vec3::X,
            0.0,
            &EffectSettings::default(),
        );
        assert!(!fx.particles.is_empty());
        let before: Vec<_> = fx.particles.iter().map(|p| (p.pos, p.floor)).collect();
        app.insert_resource(fx);
        let shift = WorldShift::translate(DVec3::new(-1000.0, 20.0, 0.0));
        app.world.write_message(shift);
        app.update();
        let fx = app.world.resource::<Effects>();
        for (p, (pos, floor)) in fx.particles.iter().zip(before) {
            assert!(p.pos.distance(pos + shift.translation) < 1e-9);
            assert!((p.floor - (floor + 20.0)).abs() < 1e-9);
        }
    }
}
