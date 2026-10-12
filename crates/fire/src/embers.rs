//! Embers (firebrands): glowing bits lofted by a fire's updraft, carried by the wind, and
//! sometimes starting new fires where they land ("spotting"), which is how real fires jump
//! roads and rivers.

use engine_core::math::{DVec3, Vec3};
use engine_core::prelude::*;

/// One ember.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ember {
    pub position: DVec3,
    pub velocity: Vec3,
    /// 0..1, falls as it cools; it can only light fuel while hot.
    pub heat: f32,
    /// Seconds it has flown.
    pub age: f32,
    /// Seconds of updraft lift left.
    pub lift: f32,
    /// Size, meters.
    pub size: f32,
    /// Seconds until it burns out.
    pub life: f32,
    /// Can start a fire where it lands (sparks from a campfire ring can't).
    pub ignites: bool,
}

/// All embers in flight.
#[derive(Resource, Debug, Clone, Default)]
pub struct Embers {
    pub list: Vec<Ember>,
    /// Where embers landed hot enough to try lighting fuel (drained by the fire step).
    pub landings: Vec<(DVec3, f32)>,
}

impl Embers {
    /// Advance every ember by `dt`. `wind(p)` gives the air velocity, `ground(p)` the ground
    /// height under a point (or `None` for no ground).
    pub fn step(
        &mut self,
        dt: f32,
        gravity: Vec3,
        wind: impl Fn(DVec3) -> Vec3,
        ground: impl Fn(DVec3) -> Option<f64>,
    ) {
        let up = (-gravity).normalize_or(Vec3::Y);
        let g = gravity.length();
        let mut i = 0;
        while i < self.list.len() {
            let e = &mut self.list[i];
            e.age += dt;
            e.heat -= dt / e.life.max(0.1);
            let mut air = wind(e.position);
            if e.lift > 0.0 {
                // Riding the plume: rises with the hot gas.
                air += up * (2.0 + 5.0 * (e.lift / 2.0).min(1.0));
                e.lift -= dt;
            }
            // Light flakes follow the air closely and fall at a few m/s terminal velocity.
            let k = 1.0 - (-3.0 * dt / e.size.max(0.005).sqrt() * 0.1).exp();
            e.velocity += (air - e.velocity) * k.clamp(0.0, 1.0);
            e.velocity -= up * g * 0.25 * dt;
            e.position += (e.velocity * dt).as_dvec3();
            let landed = ground(e.position).is_some_and(|h| e.position.y <= h);
            if landed && e.heat > 0.0 && e.ignites {
                self.landings.push((e.position, e.heat));
            }
            if landed || e.heat <= 0.0 {
                self.list.swap_remove(i);
            } else {
                i += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embers_rise_drift_downwind_and_land() {
        let mut embers = Embers::default();
        embers.list.push(Ember {
            position: DVec3::new(0.0, 1.0, 0.0),
            velocity: Vec3::ZERO,
            heat: 1.0,
            age: 0.0,
            lift: 2.0,
            size: 0.02,
            life: 30.0,
            ignites: true,
        });
        let mut max_y: f64 = 0.0;
        for _ in 0..2000 {
            embers.step(
                0.02,
                Vec3::new(0.0, -9.81, 0.0),
                |_| Vec3::new(6.0, 0.0, 0.0),
                |_| Some(0.0),
            );
            if let Some(e) = embers.list.first() {
                max_y = max_y.max(e.position.y);
            }
        }
        assert!(embers.list.is_empty());
        assert_eq!(embers.landings.len(), 1);
        let (p, heat) = embers.landings[0];
        assert!(max_y > 5.0, "embers should loft, max {max_y}");
        assert!(p.x > 15.0, "embers should drift downwind, landed at {p:?}");
        assert!(heat > 0.0);
    }
}
