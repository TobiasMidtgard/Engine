//! The demo scenery's colliders: nothing solid stands in the way of the racing line, the
//! drift circle, the runway or the landing pad, a car that runs wide is stopped by the
//! gravel before the tire wall, and one that hits the wall stays on the track side of it.
//! Physics comes from the stand-in solver in `support`.

mod support;

use engine_core::math::{DVec3, Vec2, Vec3};
use engine_core::prelude::*;
use engine_vehicles::models::scenery::solid::Solid;
use engine_vehicles::models::scenery::{airfield, spaceport, track};
use engine_vehicles::{Autopilot, demo, presets};
use support::*;

/// A solid's horizontal footprint (convex, counter-clockwise in x, z) and height range.
fn footprint(s: &Solid) -> (Vec<Vec2>, f32, f32) {
    let corners = s.corners();
    let y0 = corners.iter().map(|p| p.y).fold(f32::MAX, f32::min);
    let y1 = corners.iter().map(|p| p.y).fold(f32::MIN, f32::max);
    let mut pts: Vec<Vec2> = corners.iter().map(|p| Vec2::new(p.x, p.z)).collect();
    pts.sort_by(|a, b| a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y)));
    pts.dedup_by(|a, b| a.distance(*b) < 1e-5);
    let cross = |o: Vec2, a: Vec2, b: Vec2| (a - o).perp_dot(b - o);
    let mut hull: Vec<Vec2> = Vec::new();
    for pass in 0..2 {
        let start = hull.len();
        let iter: Box<dyn Iterator<Item = &Vec2>> = if pass == 0 {
            Box::new(pts.iter())
        } else {
            Box::new(pts.iter().rev())
        };
        for &p in iter {
            while hull.len() >= start + 2
                && cross(hull[hull.len() - 2], hull[hull.len() - 1], p) <= 0.0
            {
                hull.pop();
            }
            hull.push(p);
        }
        hull.pop();
    }
    (hull, y0, y1)
}

/// Distance from `q` to a convex counter-clockwise polygon (0 inside).
fn distance(poly: &[Vec2], q: Vec2) -> f32 {
    let n = poly.len();
    if n == 1 {
        return poly[0].distance(q);
    }
    let mut inside = n >= 3;
    let mut best = f32::MAX;
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + 1) % n]);
        if (b - a).perp_dot(q - a) < 0.0 {
            inside = false;
        }
        let t = ((q - a).dot(b - a) / (b - a).length_squared().max(1e-9)).clamp(0.0, 1.0);
        best = best.min((a + (b - a) * t).distance(q));
    }
    if inside { 0.0 } else { best }
}

fn nearest(poly: &[Vec2], points: &[Vec2]) -> f32 {
    points
        .iter()
        .map(|&q| distance(poly, q))
        .fold(f32::MAX, f32::min)
}

#[test]
fn track_solids_leave_the_racing_line_and_drift_circle_clear() {
    let solids = track::solids();
    assert!(solids.len() > 500, "{} solids", solids.len());
    let line: Vec<Vec2> = demo::race_line(DVec3::ZERO, 1440)
        .iter()
        .map(|p| Vec2::new(p.x as f32, p.z as f32))
        .collect();
    let (pad, _) = track::DRIFT_PAD;
    let drift: Vec<Vec2> = (0..360)
        .map(|i| {
            let a = (i as f32).to_radians();
            Vec2::new(pad.x + 15.0 * a.cos(), pad.z + 15.0 * a.sin())
        })
        .collect();
    let mut overhead = 0;
    for s in &solids {
        let (poly, y0, _) = footprint(s);
        let off_line = nearest(&poly, &line);
        if s.surface == "gravel_trap" {
            // Wheels up to 0.9 m either side of the line stay on the asphalt.
            assert!(
                off_line > 1.0,
                "gravel {off_line:.2} m from the racing line"
            );
            continue;
        }
        if y0 > 5.5 {
            overhead += (off_line < 1.0) as usize;
            continue;
        }
        assert!(
            off_line > 2.0,
            "{} {:?} is {off_line:.2} m from the racing line",
            s.surface,
            s.center
        );
        let off_circle = nearest(&poly, &drift);
        assert!(
            off_circle > 3.0,
            "{} {:?} is {off_circle:.2} m from the drift circle",
            s.surface,
            s.center
        );
    }
    // The start gantry's truss, the footbridge's deck, trusses and roof.
    assert!(overhead >= 5, "{overhead} solids span the track overhead");
}

#[test]
fn airfield_and_spaceport_solids_keep_runway_pad_and_stand_clear() {
    for s in airfield::solids(1200.0) {
        let (poly, _, y1) = footprint(&s);
        let near = poly.iter().map(|p| p.x.abs()).fold(f32::MAX, f32::min);
        let sides = poly.iter().filter(|p| p.x > 0.0).count();
        assert!(
            sides == 0 || sides == poly.len(),
            "{:?} straddles the runway",
            s.center
        );
        // Runway half width 22.5 m plus a 7.5 m shoulder; the circuit is flown at 140 m.
        assert!(
            near > 30.0,
            "{:?} stands {near:.1} m off the runway",
            s.center
        );
        assert!(y1 < 100.0, "{:?} reaches {y1:.0} m", s.center);
    }
    for s in spaceport::solids() {
        let (poly, _, _) = footprint(&s);
        let pad = distance(&poly, Vec2::ZERO);
        assert!(
            pad > 31.0,
            "{:?} is {pad:.1} m from the pad center",
            s.center
        );
        let stand = distance(&poly, Vec2::new(58.0, -45.0));
        assert!(
            stand > 15.0,
            "{:?} is {stand:.1} m from the parked ship",
            s.center
        );
    }
}

/// The demo track's colliders (scenery included) without its racers.
fn track_app() -> App {
    let mut app = vehicle_app();
    calm(&mut app);
    spawn_with(&mut app, |c| {
        demo::spawn_track(c, DVec3::ZERO);
        Entity::PLACEHOLDER
    });
    let racers: Vec<Entity> = app
        .world
        .query_filtered::<Entity, With<Autopilot>>()
        .iter(&app.world)
        .collect();
    for e in racers {
        app.world.despawn(e);
    }
    app
}

/// A sports car on the track's center line at oval `angle`, heading `dir` at `speed`.
fn rolling_car(app: &mut App, angle: f64, dir: Vec3, speed: f32) -> Entity {
    let at = demo::oval_point(DVec3::ZERO, angle) + DVec3::Y * presets::ride_height("sports_car");
    let t = Transform::from_translation(at).with_rotation(demo::facing(dir));
    let car = spawn_with(app, |c| presets::sports_car(c, t, 0x1d4fb8));
    set_velocity(app, car, dir * speed);
    engine_vehicles::wheels::set_rolling(&mut app.world, car, speed);
    car
}

#[test]
fn a_car_that_runs_wide_is_stopped_by_the_gravel_before_the_tires() {
    let mut app = track_app();
    // Off the outside of the east turn's apex, straight out across the curb at 11 m/s.
    let car = rolling_car(&mut app, 0.0, Vec3::X, 11.0);
    let mut furthest = 0.0f64;
    let stopped = run_until(&mut app, 12.0, |app| {
        furthest = furthest.max(transform(app, car).translation.x);
        velocity(app, car).linear.length() < 0.2
    });
    let x = transform(&app, car).translation.x - demo::TRACK_RADII.0;
    assert!(stopped.is_some(), "still rolling at {x:.1} m out");
    // The gravel starts 7.35 m out; the tire wall's belt 21.6 m out at the apex. The car's
    // nose is about 2.2 m ahead of its center.
    assert!(x > 7.35, "stopped at {x:.1} m, before the gravel");
    assert!(
        furthest - demo::TRACK_RADII.0 + 2.2 < 21.5,
        "reached {:.1} m out, into the tire wall",
        furthest - demo::TRACK_RADII.0
    );
}

#[test]
fn a_car_that_hits_the_outer_wall_stays_on_the_track_side() {
    let mut app = track_app();
    // On the main straight (driving +x), turned 30° toward the outer wall (−z) at 20 m/s.
    let dir = Vec3::new(30f32.to_radians().cos(), 0.0, -30f32.to_radians().sin());
    let car = rolling_car(&mut app, 1.5 * std::f64::consts::PI, dir, 20.0);
    let mut deepest = 0.0f64;
    run_until(&mut app, 3.0, |app| {
        deepest = deepest.max(-transform(app, car).translation.z - demo::TRACK_RADII.1);
        false
    });
    // The wall's traffic face is 9.7 m out; the car is 0.95 m wide either side of center.
    assert!(
        deepest < 9.7 - 0.6,
        "the car's center went {deepest:.2} m out"
    );
    let v = velocity(&app, car).linear;
    assert!(
        v.z > -1.0,
        "still heading out through the wall at {:.1} m/s",
        -v.z
    );
    assert!(v.length() < 19.0, "no speed lost: {:.1} m/s", v.length());
}
