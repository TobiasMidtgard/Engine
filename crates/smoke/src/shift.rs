//! Following the world when it is re-anchored: a floating-origin move, or a new reference frame
//! (`engine_world::WorldShift`). engine_world moves the entities (volumes, sources, fans,
//! obstacles), the wind and pending `SmokeEmission`s and `ProjectileTrace`s; this moves what
//! smoke keeps in world space of its own, in `WorldShiftSet::Apply`, and hands `Obscurance` the
//! moved smoke in `WorldShiftSet::Rebuild`, so the same cloud hangs over the same ground in the
//! new frame.
//!
//! * Volumes move their origin (exact, at any distance). When the world turns, a volume can't
//!   (volumes lie along the world axes): its air is resampled into a box of the same size
//!   around where it went, velocities turned, and its floor is measured again at the next step.
//!   A `SmokeVolume`'s box stays where its entity's `Transform` puts it, so it is kept.
//! * Explosions and released gas still expanding, bullet tunnels, queued API puffs, obstacles'
//!   last positions and the demo cart's track move with the world.

use std::sync::Arc;

use engine_core::math::DVec3;
use engine_core::prelude::*;
use engine_world::{Obscurance, WorldShift};

use crate::demo::DemoCart;
use crate::obscurance::{SOURCE, SmokeSnapshot};
use crate::settings::SmokeSettings;
use crate::sim::{SmokeCommand, SmokeCommands, SmokeDomains, SmokeObstacleTrack, SmokeTunnels};

/// All shifts written this frame, combined (they apply in order).
fn combined(shifts: &mut MessageReader<WorldShift>) -> Option<WorldShift> {
    shifts.read().copied().reduce(|acc, next| acc.then(&next))
}

/// Move the volumes, tunnels, queued puffs, obstacle tracks and demo carts.
pub(crate) fn shift_smoke(
    mut shifts: MessageReader<WorldShift>,
    mut domains: ResMut<SmokeDomains>,
    mut tunnels: ResMut<SmokeTunnels>,
    mut queued: ResMut<SmokeCommands>,
    mut tracks: Query<&mut SmokeObstacleTrack>,
    mut carts: Query<&mut DemoCart>,
) {
    let Some(s) = combined(&mut shifts) else {
        return;
    };
    for d in &mut domains.list {
        let size = d.domain.size();
        // A `SmokeVolume`'s box hangs from its entity at the middle of its floor; one made
        // around released smoke turns about its middle.
        let anchor = if d.owner.is_some() {
            DVec3::new(size.x * 0.5, 0.0, size.z * 0.5)
        } else {
            size * 0.5
        };
        d.domain.shift(&s, anchor);
    }
    for t in &mut tunnels.list {
        t.from = s.point(t.from);
        t.to = s.point(t.to);
    }
    for c in &mut queued.0 {
        if let SmokeCommand::Emit(e, _) = c {
            e.position = s.point(e.position);
            e.velocity = s.velocity_at(e.position, e.velocity);
        }
    }
    for mut t in &mut tracks {
        t.shift(&s);
    }
    for mut cart in &mut carts {
        cart.from = s.point(cart.from);
        cart.to = s.point(cart.to);
    }
}

/// After a shift, hand [`Obscurance`] the moved smoke, so sight lines are answered in the new
/// frame from `WorldShiftSet::Rebuild` on (the regular publish only runs after a step).
pub(crate) fn publish_shifted(
    mut shifts: MessageReader<WorldShift>,
    settings: Res<SmokeSettings>,
    domains: Res<SmokeDomains>,
    mut obscurance: ResMut<Obscurance>,
) {
    if shifts.read().count() == 0 {
        return;
    }
    obscurance.set(
        SOURCE,
        Arc::new(SmokeSnapshot::of(&domains, settings.extinction_m2_kg)),
    );
}

#[cfg(test)]
mod tests {
    use engine_core::math::{DQuat, DVec3, Vec3};
    use engine_core::prelude::*;
    use engine_world::{Obscurance, ProjectileTrace, SmokeEmission, WorldShift, WorldShiftSet};

    use crate::{SmokeDomains, SmokePlugin, SmokeSettings, SmokeTunnels, SmokeVolume};

    /// Shifts to write in the next `WorldShiftSet::Write`, and what the rest of the engine saw
    /// of the smoke right then.
    #[derive(Resource, Default)]
    struct Plan {
        shifts: Vec<WorldShift>,
        before: Option<(Obscurance, SmokeDomains, SmokeTunnels)>,
    }

    fn write_planned(world: &mut World) {
        let shifts = std::mem::take(&mut world.resource_mut::<Plan>().shifts);
        if shifts.is_empty() {
            return;
        }
        let before = (
            world.resource::<Obscurance>().clone(),
            world.resource::<SmokeDomains>().clone(),
            world.resource::<SmokeTunnels>().clone(),
        );
        world.resource_mut::<Plan>().before = Some(before);
        for s in shifts {
            world.write_message(s);
        }
    }

    fn app() -> App {
        let mut app = App::new();
        app.world
            .resource_mut::<Time>()
            .set_source(engine_core::time::TimeSource::Manual {
                step_secs: 1.0 / 30.0,
            });
        app.add_plugins(SmokePlugin);
        app.init_resource::<Plan>();
        app.add_systems(PostUpdate, write_planned.in_set(WorldShiftSet::Write));
        app.world.resource_mut::<engine_world::WeatherState>().wind = Vec3::ZERO;
        // Thin enough that a sight line through the puff is neither clear nor blind.
        app.world.resource_mut::<SmokeSettings>().extinction_m2_kg = 60.0;
        app
    }

    /// A cool puff of smoke, settled in for a few steps.
    fn puff(app: &mut App, at: DVec3) {
        app.world.write_message(SmokeEmission {
            position: at,
            velocity: Vec3::ZERO,
            radius: 1.5,
            mass_kg: 0.05,
            temperature_c: 15.0,
            darkness: 0.2,
        });
        app.update_n(6);
    }

    /// An explicit volume over the origin with a puff in it.
    fn volume(app: &mut App) -> DVec3 {
        app.world.spawn((
            Transform::from_xyz(0.0, 0.0, 0.0),
            SmokeVolume {
                size: [16.0, 8.0, 16.0],
                cell_m: 0.5,
            },
        ));
        app.update();
        let at = DVec3::new(0.7, 2.0, -0.4);
        puff(app, at);
        let domains = app.world.resource::<SmokeDomains>();
        assert!(domains.list.len() == 1 && domains.list[0].owner.is_some());
        at
    }

    /// Shift in the next frame; what was seen just before it.
    fn shift(app: &mut App, s: WorldShift) -> (Obscurance, SmokeDomains, SmokeTunnels) {
        app.world.resource_mut::<Plan>().shifts.push(s);
        app.update();
        app.world
            .resource_mut::<Plan>()
            .before
            .take()
            .expect("the shift went out")
    }

    /// Sight lines through the puff at `at` (and one through clear air beside it).
    fn sight_lines(at: DVec3) -> [(DVec3, DVec3); 4] {
        [
            (at - DVec3::X * 6.0, at + DVec3::X * 6.0),
            (
                at + DVec3::new(-4.0, -1.5, -3.0),
                at + DVec3::new(4.0, 1.0, 3.5),
            ),
            (
                at + DVec3::new(0.3, 4.0, 0.2),
                at + DVec3::new(-0.2, -1.9, 0.1),
            ),
            (
                at + DVec3::new(-6.0, 0.0, 5.0),
                at + DVec3::new(6.0, 0.0, 5.0),
            ),
        ]
    }

    /// Optical depth seen through `obscurance` from `a` to `b`.
    fn depth(obscurance: &Obscurance, a: DVec3, b: DVec3) -> f32 {
        -obscurance.transmittance(a, b).ln()
    }

    /// The smoke the engine sees after the shift is the smoke it saw before, moved.
    fn assert_moved(app: &App, s: &WorldShift, before: &Obscurance, at: DVec3, tolerance: f32) {
        let after = app.world.resource::<Obscurance>();
        let lines = sight_lines(at);
        let (a, b) = lines[0];
        let t = before.transmittance(a, b);
        assert!(t > 0.02 && t < 0.9, "pick a thinner or thicker puff: {t}");
        for (a, b) in lines {
            let (old, new) = (
                before.transmittance(a, b),
                after.transmittance(s.point(a), s.point(b)),
            );
            let (d0, d1) = (depth(before, a, b), depth(after, s.point(a), s.point(b)));
            assert!(
                (new - old).abs() < tolerance || (d1 - d0).abs() < tolerance * d0,
                "{a} -> {b}: {old} before, {new} after"
            );
        }
        // And nothing is left where the smoke was.
        assert_eq!(after.transmittance(lines[0].0, lines[0].1), 1.0);
    }

    #[test]
    fn released_smoke_is_seen_at_the_same_place_after_a_translation() {
        let mut app = app();
        let at = DVec3::new(10.0, 2.0, 10.0);
        puff(&mut app, at);
        // A bullet past it leaves a tunnel.
        app.world.write_message(ProjectileTrace {
            from: at + DVec3::new(4.0, 3.0, -8.0),
            to: at + DVec3::new(4.0, 3.0, 8.0),
            radius: 0.01,
            speed: 900.0,
            shooter: None,
        });
        app.update();
        let s = WorldShift::translate(DVec3::new(1000.3, 2.0, -500.7));
        let (before, domains, tunnels) = shift(&mut app, s);
        assert_moved(&app, &s, &before, at, 1e-4);
        let after = app.world.resource::<SmokeDomains>();
        let (old, new) = (&domains.list[0].domain, &after.list[0].domain);
        assert_eq!(new.origin, old.origin + s.translation);
        assert!(new.version > old.version);
        assert_eq!(new.voxels, old.voxels);
        let (t0, t1) = (
            tunnels.list[0],
            app.world.resource::<SmokeTunnels>().list[0],
        );
        assert!((t1.from - s.point(t0.from)).length() < 1e-9);
        assert!((t1.to - s.point(t0.to)).length() < 1e-9);
        // The API answers in the new frame too.
        let v = engine_core::registry::call_api(
            &mut app.world,
            "smoke.visibility",
            serde_json::json!({
                "from": s.point(at - DVec3::X * 6.0).to_array(),
                "to": s.point(at + DVec3::X * 6.0).to_array(),
            }),
        )
        .unwrap();
        let t = v["transmittance"].as_f64().unwrap() as f32;
        let seen = before.transmittance(at - DVec3::X * 6.0, at + DVec3::X * 6.0);
        assert!((t - seen).abs() < 1e-4, "{t} vs {seen}");
    }

    #[test]
    fn a_volume_follows_its_entity_through_a_translation_and_keeps_its_smoke() {
        let mut app = app();
        let at = volume(&mut app);
        let s = WorldShift::translate(DVec3::new(1000.3, 2.0, -500.7));
        let (before, domains, _) = shift(&mut app, s);
        assert_moved(&app, &s, &before, at, 1e-4);
        // The volume isn't made afresh around the moved entity: its smoke stays.
        app.update_n(4);
        let after = app.world.resource::<SmokeDomains>();
        assert_eq!(after.list.len(), 1);
        assert_eq!(after.list[0].id, domains.list[0].id);
        let mass = domains.list[0].domain.mass_kg;
        assert!(after.list[0].domain.mass_kg > mass * 0.9, "{mass}");
        assert!(after.density(s.point(at)) > 0.0);
    }

    #[test]
    fn smoke_is_seen_at_the_same_place_after_a_quarter_turn() {
        let s = WorldShift {
            rotation: DQuat::from_rotation_y(std::f64::consts::FRAC_PI_2),
            translation: DVec3::new(37.3, 1.5, -12.9),
            ..WorldShift::IDENTITY
        };
        // Released smoke: its volume turns about its middle.
        let mut app = app();
        let at = DVec3::new(10.0, 2.0, 10.0);
        puff(&mut app, at);
        let (before, _, _) = shift(&mut app, s);
        assert_moved(&app, &s, &before, at, 1e-3);
        // An explicit volume: its box stays where its entity puts it, and keeps its smoke.
        let mut app = self::app();
        let at = volume(&mut app);
        let (before, domains, _) = shift(&mut app, s);
        assert_moved(&app, &s, &before, at, 1e-3);
        app.update_n(4);
        let after = app.world.resource::<SmokeDomains>();
        assert_eq!(after.list.len(), 1);
        assert_eq!(after.list[0].id, domains.list[0].id);
        assert!(!after.list[0].domain.floor_stale);
        assert!(after.density(s.point(at)) > 0.0);
    }

    #[test]
    fn smoke_is_seen_near_the_same_place_after_any_turn() {
        let s = WorldShift {
            rotation: DQuat::from_rotation_y(0.5) * DQuat::from_rotation_x(0.05),
            translation: DVec3::new(-250.25, 3.0, 80.1),
            ..WorldShift::IDENTITY
        };
        let mut app = app();
        let at = DVec3::new(10.0, 2.0, 10.0);
        puff(&mut app, at);
        let (before, domains, _) = shift(&mut app, s);
        // Resampling blurs the puff a little, but it is all there, where it went.
        assert_moved(&app, &s, &before, at, 0.2);
        let (old, new) = (
            &domains.list[0].domain,
            &app.world.resource::<SmokeDomains>().list[0].domain,
        );
        assert!(
            (new.mass_kg - old.mass_kg).abs() < old.mass_kg * 0.05,
            "{} -> {}",
            old.mass_kg,
            new.mass_kg
        );
        let (c0, c1) = (centroid(old), centroid(new));
        assert!(
            (c1 - s.point(c0)).length() < 0.25,
            "{c1} vs {}",
            s.point(c0)
        );
    }

    /// Center of mass of a volume's smoke.
    fn centroid(d: &crate::Domain) -> DVec3 {
        let mut sum = DVec3::ZERO;
        let mut total = 0.0;
        for z in 0..d.dims.z {
            for y in 0..d.dims.y {
                for x in 0..d.dims.x {
                    let m = d.voxels[d.index(x, y, z)].density as f64;
                    sum += d.voxel_center(x, y, z) * m;
                    total += m;
                }
            }
        }
        sum / total
    }
}
