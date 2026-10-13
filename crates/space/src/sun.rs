//! The sun as the rest of the world sees it: local solar time and the season at a spot on a
//! body, in the calendar of engine_world's [`WeatherState`] (hours 0..24, days 0..365.25 with
//! the winter solstice at day −10).
//!
//! Two ways to agree with the weather's day/night cycle:
//! * The space module lights the scene ([`StarLight`](crate::StarLight)): it writes
//!   `WeatherState::sun_direction`, `time_of_day_h` and `day_of_year` from the real sun, so
//!   clouds, sky, wildlife and sound follow it.
//! * The weather lights the scene ([`SpaceSettings::follow_world_clock`]): the space clock
//!   follows `time_of_day_h` and `day_of_year`, so the real sun stands where the weather's
//!   sun does over the home spot and the Moon and planets are lit from there.

use std::f64::consts::TAU;

use engine_core::math::{DQuat, DVec3};
use engine_core::prelude::*;
use engine_world::WeatherState;

use crate::{
    BodyKind, BodyState, HomeFrame, SpaceClock, SpaceFrame, SpaceSettings, anchor_index,
    collect_bodies, dir_from_lat_lon, propagate,
};

/// Days in the weather's year.
pub const YEAR_DAYS: f64 = 365.25;

/// Local solar time (hours, 0..24) at `point` on a body at `position` turned by `rotation`,
/// with its star at `star` (all in one space): 12 when the star crosses that meridian.
pub fn solar_hour(rotation: DQuat, position: DVec3, star: DVec3, point: DVec3) -> f64 {
    let inv = rotation.inverse();
    let (s, p) = (inv * (star - position), inv * (point - position));
    let lon = |v: DVec3| v.x.atan2(v.z).to_degrees();
    (12.0 + wrap(lon(p) - lon(s), 360.0) / 15.0).rem_euclid(24.0)
}

/// The weather's day of the year (0..365.25) for where the star stands in a body's year:
/// day −10 when it is lowest over the northern hemisphere (winter solstice), 172.6 when
/// highest. `pole` is the body's north axis, `orbit_normal` its orbit's angular momentum
/// around the star and `to_star` the direction from the body to the star.
pub fn season_day(pole: DVec3, orbit_normal: DVec3, to_star: DVec3) -> f64 {
    let n = orbit_normal.normalize_or(DVec3::Y);
    let summer = (pole - n * pole.dot(n))
        .try_normalize()
        .unwrap_or_else(|| n.any_orthonormal_vector());
    let winter = -summer;
    let d = (to_star - n * to_star.dot(n)).normalize_or(winter);
    let phase = n.dot(winter.cross(d)).atan2(winter.dot(d));
    (YEAR_DAYS * phase / TAU - 10.0).rem_euclid(YEAR_DAYS)
}

/// `v` wrapped into −period/2..period/2.
fn wrap(v: f64, period: f64) -> f64 {
    (v + period * 0.5).rem_euclid(period) - period * 0.5
}

/// The star a body circles (the root of its parents if that is a star, else the first star).
fn star_of(states: &[BodyState], mut i: usize) -> Option<usize> {
    for _ in 0..16 {
        match states[i].parent {
            Some(p) => i = p,
            None => break,
        }
    }
    (states[i].kind == BodyKind::Star)
        .then_some(i)
        .or_else(|| states.iter().position(|s| s.kind == BodyKind::Star))
}

/// A body's day under its star: solar hour at a spot, season, and the solar day's length.
struct LocalSky {
    hour: f64,
    day: f64,
    /// Seconds from one noon to the next (infinite if the star never moves in its sky).
    solar_day_s: f64,
}

impl LocalSky {
    /// At body-local direction `spot` on body `i` (inertial states).
    fn of(states: &[BodyState], i: usize, spot: DVec3) -> Option<Self> {
        let star = &states[star_of(states, i)?];
        let b = &states[i];
        if star.entity == b.entity {
            return None;
        }
        let point = b.inertial_position + b.orientation * spot;
        let r = b.inertial_position - star.inertial_position;
        let v = b.inertial_velocity - star.inertial_velocity;
        let h = r.cross(v);
        // The star crosses the sky at the spin rate less the body's own motion around it.
        let spin = b.spin.length();
        let around = h.length() / r.length_squared().max(1.0) * h.dot(b.spin).signum();
        let rate = spin - around;
        Some(Self {
            hour: solar_hour(
                b.orientation,
                b.inertial_position,
                star.inertial_position,
                point,
            ),
            day: season_day(b.orientation * DVec3::Y, h, -r),
            solar_day_s: if rate.abs() > 1e-12 {
                TAU / rate.abs()
            } else {
                f64::INFINITY
            },
        })
    }
}

/// The sun fields of [`WeatherState`] for the world origin on the anchor, with the star in
/// world direction `sun` (states in world placement, see [`crate::update_space`]).
pub(crate) fn write_weather_sun(
    world: &mut World,
    states: &[BodyState],
    anchor: usize,
    sun: DVec3,
) {
    let b = &states[anchor];
    let Some(star) = star_of(states, anchor).map(|s| &states[s]) else {
        return;
    };
    if star.entity == b.entity {
        return;
    }
    let hour = solar_hour(
        b.world_rotation,
        b.world_position,
        star.world_position,
        DVec3::ZERO,
    );
    let r = b.inertial_position - star.inertial_position;
    let day = season_day(
        b.orientation * DVec3::Y,
        r.cross(b.inertial_velocity - star.inertial_velocity),
        -r,
    );
    if let Some(mut w) = world.get_resource_mut::<WeatherState>() {
        w.sun_direction = sun.as_vec3();
        w.time_of_day_h = hour as f32;
        w.day_of_year = day as f32;
    }
}

/// FixedUpdate, with [`SpaceSettings::follow_world_clock`]: set the space clock so that over
/// the home spot (the [`HomeFrame`]'s) the real sun stands at `WeatherState::time_of_day_h`
/// local solar time on `day_of_year`. Small steps from the current clock keep it continuous,
/// so the Moon and planets carry on along their orbits as the weather's clock runs.
pub(crate) fn follow_world_clock(world: &mut World) {
    if !world.resource::<SpaceSettings>().follow_world_clock {
        return;
    }
    let Some(w) = world.get_resource::<WeatherState>() else {
        return;
    };
    let hour = w.time_of_day_h as f64;
    // The weather's date turns at local midnight; the season runs on through the day.
    let target_day = w.day_of_year as f64 + hour / 24.0;
    let bodies = collect_bodies(world);
    if bodies.is_empty() {
        return;
    }
    let home = match world.get_resource::<HomeFrame>() {
        Some(h) => h.0.clone(),
        None => world.resource::<SpaceFrame>().clone(),
    };
    let spot = dir_from_lat_lon(home.home_lat_deg, home.home_lon_deg);
    let mut t = world.resource::<SpaceClock>().seconds;
    // Hour, whole days toward the season, hour again.
    for step in 0..3 {
        let states = propagate(&bodies, t);
        let i = anchor_index(&states, &home);
        let Some(sky) = LocalSky::of(&states, i, spot) else {
            return;
        };
        if !sky.solar_day_s.is_finite() {
            return;
        }
        t += if step == 1 {
            wrap(target_day - sky.day, YEAR_DAYS).round() * sky.solar_day_s
        } else {
            wrap(hour - sky.hour, 24.0) / 24.0 * sky.solar_day_s
        };
    }
    world.resource_mut::<SpaceClock>().seconds = t;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hours_and_seasons() {
        // Star along +Z of a body at the origin: noon at longitude 0, 18:00 at 90° W.
        let r = DQuat::IDENTITY;
        let star = DVec3::Z * 1e11;
        let at = |lon: f64| dir_from_lat_lon(0.0, lon);
        assert!((solar_hour(r, DVec3::ZERO, star, at(0.0)) - 12.0).abs() < 1e-9);
        assert!((solar_hour(r, DVec3::ZERO, star, at(-90.0)) - 6.0).abs() < 1e-9);
        assert!((solar_hour(r, DVec3::ZERO, star, at(90.0)) - 18.0).abs() < 1e-9);

        // Pole leaning toward +Z, orbit normal +Y: the star toward +Z is midsummer.
        let pole = DVec3::new(0.0, 0.92, 0.4);
        let summer = season_day(pole, DVec3::Y, DVec3::Z);
        assert!((summer - (YEAR_DAYS / 2.0 - 10.0)).abs() < 1e-6, "{summer}");
        let winter = season_day(pole, DVec3::Y, -DVec3::Z);
        assert!((winter - (YEAR_DAYS - 10.0)).abs() < 1e-6, "{winter}");
        // A quarter year after the winter solstice, turning about +Y from −Z: −X.
        let spring = season_day(pole, DVec3::Y, -DVec3::X);
        assert!((spring - (YEAR_DAYS / 4.0 - 10.0)).abs() < 1e-6, "{spring}");
    }
}
