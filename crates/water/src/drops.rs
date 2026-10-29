//! Splash drops: the water a splash throws up comes back down.
//!
//! An object hitting the water at speed throws a crown of drops out round the hole it makes,
//! and a fast, small one a jet up the middle a moment later, when the air cavity behind it
//! pinches shut (Worthington, "A Study of Splashes", 1908; the pinch-off time scales with
//! √(R/g) whatever the speed: Glasheen & McMahon 1996, Duclaux et al., "Dynamics of transient
//! cavities", J. Fluid Mech. 2007). Every drop flies ballistically under gravity and air drag
//! and, where it lands on water, rings it and leaves a fleck of foam: a splash spreads as a
//! scatter of small rings round the big one, the jet's fall rings the middle again, a fast
//! hull's bow spray lays foam along its wake, and a fall's spray speckles its pool.
//!
//! The particles module draws the spray from the same [`WaterSplash`] messages; these drops
//! are what the water itself feels of it.

use engine_core::math::{DVec2, DVec3, Rng, Vec3};
use engine_core::prelude::*;
use engine_world::WaterSplash;

use crate::ripples::Disturbance;
use crate::state::SharedWater;

/// Most drops in flight at once.
const MAX_DROPS: usize = 2048;
const G: f32 = 9.81;

/// One drop in flight.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Drop {
    pub pos: DVec3,
    /// m/s.
    pub vel: Vec3,
    /// Meters.
    pub radius: f32,
    /// Seconds before it leaves the water (a jet waits for its cavity to close).
    pub delay: f32,
    /// Lowest it may fall to before it is given up on (it missed the water), world y.
    pub floor: f64,
}

/// The drops in flight.
#[derive(Resource, Debug, Default)]
pub struct SplashDrops {
    pub drops: Vec<Drop>,
}

/// The drops a splash throws: a crown round the hole, a jet up the middle after a fast
/// entry, or a spray of a few along its velocity when it is thrown out (a bow wave, a fall's
/// spray, `velocity.y > 0`).
pub fn throw(s: &WaterSplash, rng: &mut Rng) -> Vec<Drop> {
    let mut out = Vec::new();
    let floor = s.position.y - 30.0;
    let mass = s.mass_kg.max(0.0);
    let speed = s.velocity.length();
    if speed < 0.5 || mass <= 0.0 {
        return out;
    }
    let along = Vec3::new(s.velocity.x, 0.0, s.velocity.z);
    if s.velocity.y > 0.0 {
        // Thrown out: a few drops along the throw, spread a little.
        let n = (2.0 + 3.0 * mass.sqrt()).round().clamp(2.0, 8.0) as usize;
        for _ in 0..n {
            let jitter = Vec3::new(rng.f32() - 0.5, rng.f32() * 0.5, rng.f32() - 0.5) * speed * 0.5;
            let d = rng.in_unit_disk() * s.radius;
            out.push(Drop {
                pos: s.position + DVec3::new(d.x as f64, 0.05, d.y as f64),
                vel: s.velocity * (0.6 + 0.6 * rng.f32()) + jitter,
                radius: (0.003 + 0.01 * (mass / n as f32).sqrt()).min(0.02),
                delay: 0.0,
                floor,
            });
        }
        return out;
    }
    // Going in: the entry speed sets how high and far the crown goes.
    let entry = (-s.velocity.y).max(speed * 0.5);
    let n = (4.0 + 6.0 * mass.sqrt()).round().clamp(4.0, 24.0) as usize;
    let rim = s.radius.max(0.05);
    for k in 0..n {
        let turn = (k as f32 + rng.f32() * 0.8) / n as f32 * std::f32::consts::TAU;
        let out_dir = Vec3::new(turn.cos(), 0.0, turn.sin());
        // Crown sheets leave steeply, at a fraction of the entry speed.
        let climb = (50.0 + 25.0 * rng.f32()).to_radians();
        let v = (entry * (0.25 + 0.3 * rng.f32())).min(10.0);
        out.push(Drop {
            pos: s.position + (out_dir * rim).as_dvec3() + DVec3::Y * 0.05,
            vel: out_dir * v * climb.cos() + Vec3::Y * v * climb.sin() + along * 0.3,
            radius: (0.003 + 0.012 * (mass / n as f32).sqrt()).min(0.03),
            delay: 0.0,
            floor,
        });
    }
    // A small, fast object leaves a cavity that pinches shut and shoots a jet up the middle.
    if entry > 3.0 && s.radius < 0.8 {
        out.push(Drop {
            pos: s.position + DVec3::Y * 0.05,
            vel: Vec3::Y * (0.5 * entry).min(6.0) + along * 0.1,
            radius: (0.01 + 0.03 * s.radius).min(0.04),
            delay: 2.3 * (s.radius.max(0.02) / G).sqrt(),
            floor,
        });
    }
    out
}

/// Where a drop lands on water: a ring sized by the drop, deeper the faster it comes in, and
/// a fleck of foam (both calibrated against the raindrops' rings, `disturbance_system`).
pub fn landing(d: &Drop) -> Disturbance {
    let size = (d.radius / 0.01).min(3.0);
    let speed = d.vel.length();
    Disturbance {
        pos: DVec2::new(d.pos.x, d.pos.z),
        radius: (d.radius * 8.0).max(0.12),
        depth: (0.003 + 0.006 * (speed / 5.0).min(1.5)) * size.max(0.3),
        foam: (0.15 * size).min(0.4),
        stir: None,
        spin: 0.0,
        press: 0.0,
    }
}

/// Advance a drop by `dt`: gravity and quadratic drag (a 1 cm drop falls at ~9 m/s at most).
pub fn fly(d: &mut Drop, dt: f32) {
    let drag = 0.12 * (0.01 / d.radius.max(0.001));
    let v = d.vel.length();
    d.vel.y -= G * dt;
    d.vel /= 1.0 + drag * v * dt;
    d.pos += (d.vel * dt).as_dvec3();
}

/// Throw drops for every splash and fly them; those that land on water ring it.
pub fn splash_drops_system(
    time: Res<Time>,
    shared: Res<SharedWater>,
    mut splashes: MessageReader<WaterSplash>,
    mut drops: ResMut<SplashDrops>,
    mut rng: Local<Option<Rng>>,
) {
    let rng = rng.get_or_insert_with(|| Rng::new(0xD2095));
    for s in splashes.read() {
        let room = MAX_DROPS.saturating_sub(drops.drops.len());
        let thrown = throw(s, rng);
        drops.drops.extend(thrown.into_iter().take(room));
    }
    if drops.drops.is_empty() {
        return;
    }
    let dt = time.delta_secs();
    let mut landed = Vec::new();
    {
        let state = shared.0.read();
        drops.drops.retain_mut(|d| {
            if d.delay > 0.0 {
                d.delay -= dt;
                return true;
            }
            fly(d, dt);
            if d.vel.y > 0.0 {
                return true;
            }
            match state.sample(d.pos) {
                Some(w) if d.pos.y <= w.surface_height => {
                    landed.push(landing(d));
                    false
                }
                // Over dry ground (or nothing): given up on once it is well under the water
                // it left.
                Some(_) => true,
                None => {
                    let ground = state.bed_height(d.pos).unwrap_or(d.floor);
                    d.pos.y > ground.max(d.floor)
                }
            }
        });
    }
    if landed.is_empty() {
        return;
    }
    let mut state = shared.0.write();
    for d in landed {
        state.ripples.disturb(d);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(speed: f32, radius: f32, mass: f32) -> WaterSplash {
        WaterSplash {
            position: DVec3::ZERO,
            velocity: Vec3::new(0.0, -speed, 0.0),
            radius,
            mass_kg: mass,
            source: None,
        }
    }

    /// Fly a drop until it comes back down to y = 0; (where, when).
    fn land(mut d: Drop) -> (DVec3, f32) {
        let dt = 1.0 / 120.0;
        let mut t = d.delay;
        loop {
            fly(&mut d, dt);
            t += dt;
            if d.vel.y < 0.0 && d.pos.y <= 0.0 {
                return (d.pos, t);
            }
            assert!(t < 10.0);
        }
    }

    #[test]
    fn a_faster_entry_throws_the_crown_further() {
        let mut rng = Rng::new(1);
        let reach = |speed: f32, rng: &mut Rng| {
            let drops = throw(&entry(speed, 0.1, 2.0), rng);
            let crown: Vec<_> = drops.iter().filter(|d| d.delay == 0.0).collect();
            assert!(crown.len() >= 4);
            crown
                .iter()
                .map(|d| DVec2::new(land(**d).0.x, land(**d).0.z).length())
                .sum::<f64>()
                / crown.len() as f64
        };
        let slow = reach(3.0, &mut rng);
        let fast = reach(12.0, &mut rng);
        assert!(fast > 1.5 * slow, "slow {slow} fast {fast}");
        // A stone thrown hard lands its crown a meter or two out, not tens of meters.
        assert!(fast > 0.3 && fast < 6.0, "{fast}");
    }

    #[test]
    fn a_fast_small_entry_shoots_a_jet_that_falls_back_in_the_middle() {
        let mut rng = Rng::new(2);
        let drops = throw(&entry(8.0, 0.05, 0.5), &mut rng);
        let jet = drops.iter().find(|d| d.delay > 0.0).expect("a jet");
        let (at, when) = land(*jet);
        assert!(DVec2::new(at.x, at.z).length() < 0.1, "{at:?}");
        // It rises for a few tenths of a second after the cavity closes.
        assert!(when > 0.4 && when < 1.5, "{when}");
        // A big, slow body makes no jet.
        assert!(
            throw(&entry(2.0, 1.5, 50.0), &mut rng)
                .iter()
                .all(|d| d.delay == 0.0)
        );
    }

    #[test]
    fn landings_ring_like_rain_and_fleck_foam() {
        let d = Drop {
            pos: DVec3::new(1.0, 0.0, 2.0),
            vel: Vec3::new(0.0, -4.0, 0.0),
            radius: 0.01,
            delay: 0.0,
            floor: -30.0,
        };
        let l = landing(&d);
        assert_eq!(l.pos, DVec2::new(1.0, 2.0));
        // Raindrops push 4-14 mm.
        assert!(l.depth > 0.004 && l.depth < 0.014, "{}", l.depth);
        assert!(l.foam > 0.0 && l.foam < 0.5);
    }
}
