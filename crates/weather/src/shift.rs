//! Following world shifts (a floating-origin move, or a turned reference frame): the fronts,
//! the cloud pattern, the sky and the puddles stay over the same ground. engine_world moves the
//! shared state (`WeatherState`'s wind and sun, the wind field, the surface grid, root
//! transforms such as the sun and moon lights and the station); this moves the weather's own.

use engine_core::math::{DVec2, DVec3};
use engine_core::prelude::*;
use engine_world::WorldShift;

use crate::cloudmap::{self, CloudMap};
use crate::lightning::{ActiveBolts, StrikeRequests};
use crate::sim::WeatherSim;
use crate::sky::SkyLights;
use crate::surface::GroundCache;

/// PostUpdate, in `WorldShiftSet::Apply`.
pub(crate) fn shift_system(
    mut shifts: MessageReader<WorldShift>,
    mut sim: ResMut<WeatherSim>,
    mut map: ResMut<CloudMap>,
    mut cache: ResMut<GroundCache>,
    mut bolts: ResMut<ActiveBolts>,
    mut requests: ResMut<StrikeRequests>,
    mut lights: ResMut<SkyLights>,
) {
    let mut shifted = false;
    for s in shifts.read() {
        shifted = true;
        sim.frame.follow(s);
        // The planet's cloud systems stay where they were until their writer rebuilds the
        // field on the new axes (dropping it moved every far cloud at once).
        sim.field_moved = sim.field_moved.then(s);
        let focus = s.point(DVec3::new(sim.focus.x, 0.0, sim.focus.y));
        sim.focus = DVec2::new(focus.x, focus.z);
        for bolt in &mut bolts.bolts {
            for seg in &mut bolt.segments {
                seg.a = s.point(seg.a);
                seg.b = s.point(seg.b);
            }
            bolt.light_at = s.point(bolt.light_at);
            bolt.cloud_at = s.point(bolt.cloud_at);
        }
        for at in &mut requests.0 {
            *at = s.point(*at);
        }
        // Until the sky clock next runs.
        lights.sun_dir = s.direction(lights.sun_dir);
        lights.moon_dir = s.direction(lights.moon_dir);
        if s.is_translation_only() {
            // engine_world moved the surface grid's origin the same way, so the cache stays
            // laid out like it.
            let t = s.translation;
            cache.cells.origin += DVec2::new(t.x, t.z);
            for c in &mut cache.cells.data {
                c.height += t.y as f32;
            }
            cache.heights_changed();
            // Roof lookups in flight were measured in the old frame.
            cache.epoch += 1;
        } else {
            // The surface grid was resampled onto new axes: gather the ground facts again
            // (nearest the player first).
            *cache = GroundCache::default();
        }
    }
    if shifted {
        // The map is laid out on world axes.
        cloudmap::build(&mut map, &sim);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::{Weather, WeatherFrame};
    use crate::{WeatherPlugin, WeatherPreset};
    use engine_core::math::{DQuat, Vec3};

    fn turned(yaw: f64, t: DVec3) -> WorldShift {
        WorldShift {
            rotation: DQuat::from_rotation_y(yaw),
            translation: t,
            ..WorldShift::IDENTITY
        }
    }

    #[test]
    fn the_pattern_frame_keeps_ground_points() {
        let mut frame = WeatherFrame::default();
        let points = [
            DVec2::new(0.0, 0.0),
            DVec2::new(1200.0, -300.0),
            DVec2::new(-45_000.0, 8_000.0),
        ];
        let mut world = points;
        for s in [
            WorldShift::translate(DVec3::new(-5000.0, 20.0, 1300.0)),
            turned(0.7, DVec3::new(300.0, 0.0, -90.0)),
            turned(-2.1, DVec3::new(-40_000.0, -5.0, 2.0)),
        ] {
            frame.follow(&s);
            for (w, original) in world.iter_mut().zip(points) {
                let moved = s.point(DVec3::new(w.x, 0.0, w.y));
                *w = DVec2::new(moved.x, moved.z);
                assert!(
                    (frame.to_pattern(*w) - original).length() < 1e-6,
                    "{w} → {} (was {original})",
                    frame.to_pattern(*w)
                );
                assert!((frame.to_world(original) - *w).length() < 1e-6);
            }
        }
        // Directions turn with the world.
        let s = turned(0.9, DVec3::ZERO);
        let mut frame = WeatherFrame::default();
        frame.follow(&s);
        let east = Vec3::new(-1.0, 0.0, 0.0);
        assert!((frame.dir3_to_world(east) - s.direction(east)).length() < 1e-5);
    }

    #[test]
    fn the_weather_stays_over_the_ground_when_the_world_shifts() {
        let mut app = App::new();
        app.add_plugins((engine_world::WorldPlugin, WeatherPlugin));
        app.world
            .resource_mut::<Time>()
            .set_source(engine_core::time::TimeSource::Manual { step_secs: 0.1 });
        app.world.resource_mut::<Weather>().preset = WeatherPreset::PartlyCloudy;
        app.world.resource_mut::<Weather>().wind_from_deg = 250.0;
        app.update_n(3);
        // A storm on the way, partway across.
        {
            let mut w = app.world.resource_mut::<Weather>();
            w.preset = WeatherPreset::Storm;
            w.transition_s = 900.0;
        }
        app.update_n(20);
        let probes = [DVec2::new(0.0, 0.0), DVec2::new(8000.0, -2500.0)];
        let before: Vec<_> = {
            let sim = app.world.resource::<WeatherSim>();
            probes.iter().map(|p| sim.cell_at(*p)).collect()
        };
        let wind_before = app.world.resource::<engine_world::WeatherState>().wind;

        let s = turned(0.6, DVec3::new(-12_000.0, 0.0, 3_000.0));
        app.world.write_message(s);
        app.update();

        let sim = app.world.resource::<WeatherSim>();
        for (p, b) in probes.iter().zip(&before) {
            let moved = s.point(DVec3::new(p.x, 0.0, p.y));
            let after = sim.cell_at(DVec2::new(moved.x, moved.z));
            // One frame of drift and front travel aside, the same weather.
            assert!(
                (after.coverage - b.coverage).abs() < 0.02,
                "{p}: {} vs {}",
                after.coverage,
                b.coverage
            );
        }
        // The wind turned with the world and stays turned once the weather steps again.
        app.update_n(2);
        let wind = app.world.resource::<engine_world::WeatherState>().wind;
        let expected = s.direction(wind_before);
        assert!(
            wind.normalize().dot(expected.normalize()) > 0.99,
            "{wind} vs {expected}"
        );
    }

    fn shifting_app() -> App {
        let mut app = App::new();
        app.add_plugins((engine_world::WorldPlugin, WeatherPlugin));
        app.world
            .resource_mut::<Time>()
            .set_source(engine_core::time::TimeSource::Manual { step_secs: 0.1 });
        app.world.resource_mut::<Weather>().preset = WeatherPreset::PartlyCloudy;
        // The player, who moves with the world.
        app.world.spawn((
            Transform::default(),
            GlobalTransform::default(),
            engine_world::SimulationFocus::default(),
        ));
        app.update_n(3);
        app
    }

    #[test]
    fn the_planets_cloud_systems_stay_put_through_a_turned_shift() {
        let mut app = shifting_app();
        // Systems laid out by the planet: cover rising from west to east, 128 km across.
        let size = 128u32;
        let coverage: Vec<u8> = (0..size * size).map(|i| ((i % size) * 2) as u8).collect();
        let field = engine_world::CloudCoverMap {
            origin: DVec2::splat(-64_000.0),
            cell_m: 1000.0,
            size,
            coverage: coverage.into(),
            background: 0.5,
            revision: 7,
            ..Default::default()
        };
        app.world
            .insert_resource(engine_world::CloudCoverField(field));
        app.update_n(2);
        let far = [DVec2::new(45_000.0, 0.0), DVec2::new(-30_000.0, 35_000.0)];
        let cover = |app: &App, p: DVec2| {
            let sim = app.world.resource::<WeatherSim>();
            sim.base_cover(sim.base.cloud_cover, p)
        };
        let before: Vec<f32> = far.iter().map(|p| cover(&app, *p)).collect();

        // The writer hasn't rebuilt the field on the new axes yet.
        let s = turned(1.1, DVec3::new(-9_000.0, 0.0, 4_000.0));
        app.world.write_message(s);
        app.update_n(3);
        for (p, b) in far.iter().zip(&before) {
            let moved = s.point(DVec3::new(p.x, 0.0, p.y));
            let after = cover(&app, DVec2::new(moved.x, moved.z));
            assert!((after - b).abs() < 0.02, "{p}: {after} vs {b}");
        }

        // Its rebuild is on today's axes: read as it is.
        let mut rebuilt = app
            .world
            .resource::<engine_world::CloudCoverField>()
            .0
            .clone();
        rebuilt.revision += 1;
        app.world
            .insert_resource(engine_world::CloudCoverField(rebuilt));
        app.update();
        assert_eq!(
            app.world.resource::<WeatherSim>().field_moved,
            WorldShift::IDENTITY
        );
    }

    #[test]
    fn a_rebuild_in_progress_never_lands_on_the_old_axes() {
        let mut app = shifting_app();
        let mut sign = 1.0;
        // Shift at several points of the rebuild cycle.
        for wait in [1, 2, 3, 5, 7] {
            app.update_n(wait);
            sign = -sign;
            app.world
                .write_message(WorldShift::translate(DVec3::new(sign * 40_000.0, 0.0, 0.0)));
            for _ in 0..12 {
                app.update();
                let map = app.world.resource::<CloudMap>();
                let sim = app.world.resource::<WeatherSim>();
                let span = map.size as f64 * map.cell_m as f64;
                let center = map.origin + DVec2::splat(span * 0.5);
                assert!(
                    (center - sim.focus).length() < span * 0.05,
                    "map centered at {center}, focus {}",
                    sim.focus
                );
            }
        }
    }
}
