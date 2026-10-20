//! Coasting through time warp (patched conics). While the space clock runs faster than real
//! time, physics can't keep up with a ship in orbit, so the space module flies every free
//! physics body that is out in space on its exact Kepler path instead ([`OnRails`]): around
//! the body whose sphere of influence holds it, switching to the next body where it crosses a
//! sphere's edge (so a ship sent toward the Moon swings past it, or falls into its orbit), and
//! handing it back to physics, with the same position and motion, when the warp drops.
//!
//! A ship that coasts down into a body's air (or into low space over an airless body) drops the
//! warp back to real time there, so physics flies the reentry or the landing.
//!
//! Engines still work on the rails: a railed ship's `ExternalForce` (over its mass) bends its
//! path through the sped-up time, in short steps, and turning the ship (its `Transform`)
//! points the burn, so a game can fly maneuvers at any warp.

use std::collections::HashSet;

use engine_core::math::{DQuat, DVec3, Quat, Vec3};
use engine_core::prelude::*;
use engine_world::physics::{
    BodyKind as Motion, ExternalForce, MassProperties, RigidBody, Velocity,
};

use crate::conic::Conic;
use crate::orbit::Orbit;
use crate::{
    BodyKind, BodyState, CelestialBody, FrameRef, Parked, SpaceClock, SpaceFrame, SpaceSettings,
    SpaceState, anchor_index, collect_bodies, frame_for, propagate,
};

/// A ship (or any free body) the space module is flying on its Kepler path through time warp.
/// The module puts it on and takes it off; read it for maps and flight displays.
#[derive(Component, Debug, Clone, PartialEq)]
pub struct OnRails {
    /// The body it falls around.
    pub around: String,
    /// Its path around that body, in inertial axes (see [`SpaceState::to_inertial`]), from
    /// where it is now.
    pub conic: Conic,
    /// Its attitude in inertial axes, held while it coasts.
    pub attitude: DQuat,
    /// Space clock time it has been flown to.
    checked: f64,
    /// The world-from-inertial turn it was placed with, and the rotation it was given, so a
    /// turn the game makes between frames is kept.
    placed: (DQuat, Quat),
}

/// Keeps a body off the rails (something the game moves itself while time is sped up).
#[derive(Component, Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct NoRails;

/// How coasting went (for tests, tools and flight displays).
#[derive(Resource, Debug, Clone, Default)]
pub struct RailsLog {
    /// Times a coasting ship reached air or low space and dropped the warp to real time.
    pub warp_drops: u64,
    /// Switches from one body's sphere of influence to another's.
    pub handoffs: u64,
    /// Space clock rate seen last frame (space seconds per world second).
    pub rate: f64,
    last_clock: Option<f64>,
}

/// One body as the rails see it.
struct Body {
    name: String,
    mu: f64,
    soi: f64,
    /// Distance from the center below which coasting stops (top of the air, or low space).
    floor: f64,
    parent: Option<usize>,
    children: Vec<usize>,
    orbit: Orbit,
}

impl Body {
    /// Position and velocity relative to its parent at `t` (as [`propagate`] has it).
    fn relative(&self, parent_mu: f64, t: f64) -> (DVec3, DVec3) {
        self.orbit.state_at(parent_mu + self.mu, t)
    }
}

fn table(bodies: &[(Entity, CelestialBody)], states: &[BodyState]) -> Vec<Body> {
    let mut list: Vec<Body> = bodies
        .iter()
        .zip(states)
        .map(|((_, b), s)| {
            let air = b
                .air
                .map_or(0.0, |a| a.top_m as f64)
                .max(b.atmosphere.map_or(0.0, |a| a.thickness_m as f64));
            let floor = if b.kind == BodyKind::Star {
                b.radius_m * 2.0
            } else {
                b.radius_m + air.max(b.radius_m * 0.008)
            };
            Body {
                name: b.name.clone(),
                mu: b.mu,
                soi: s.soi_m,
                floor,
                parent: s.parent,
                children: Vec::new(),
                orbit: b.orbit,
            }
        })
        .collect();
    for i in 0..list.len() {
        if let Some(p) = list[i].parent {
            list[p].children.push(i);
        }
    }
    list
}

enum Event {
    /// Down into the air or low space.
    Floor,
    /// Out of the body's sphere of influence, into its parent's.
    Leave,
    /// Into a moon's (or planet's) sphere of influence.
    Enter(usize),
}

/// A sphere of influence is entered a little inside its edge, so a ship that just left one
/// doesn't fall straight back in.
const ENTER: f64 = 0.995;

/// The first time in `lo..hi` where `hit` holds (it doesn't at `lo` and does at `hi`).
fn bisect(mut lo: f64, mut hi: f64, hit: impl Fn(f64) -> bool) -> f64 {
    for _ in 0..80 {
        if hi - lo <= 1e-3 {
            break;
        }
        let mid = 0.5 * (lo + hi);
        if hit(mid) {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    hi
}

/// The first event on `conic` around body `p` after `t0`, up to `t1`.
fn next_event(conic: &Conic, p: usize, bodies: &[Body], t0: f64, t1: f64) -> Option<(f64, Event)> {
    let b = &bodies[p];
    let (rp, ra) = (conic.periapsis(), conic.apoapsis());
    let floor = rp < b.floor;
    let leave = b.soi.is_finite() && ra > b.soi;
    let moons: Vec<usize> = b
        .children
        .iter()
        .copied()
        .filter(|&c| {
            let o = &bodies[c].orbit;
            let reach = bodies[c].soi;
            let (near, far) = (
                o.semi_major_axis_m * (1.0 - o.eccentricity),
                o.semi_major_axis_m * (1.0 + o.eccentricity),
            );
            ra >= near - reach && rp <= far + reach
        })
        .collect();
    if !floor && !leave && moons.is_empty() || t1 <= t0 {
        return None;
    }
    // Steps short enough that each holds at most one turn point of the distance, and that the
    // ship moves no more than a quarter of a moon's sphere against it.
    let mut h = t1 - t0;
    if let Some(period) = conic.period() {
        h = h.min(period / 8.0);
    }
    let fastest = conic.max_speed();
    for &c in &moons {
        let o = &bodies[c].orbit;
        let near = (o.semi_major_axis_m * (1.0 - o.eccentricity)).max(1.0);
        let speed = (b.mu * (2.0 / near - 1.0 / o.semi_major_axis_m))
            .max(0.0)
            .sqrt();
        h = h.min(0.25 * bodies[c].soi / (fastest + speed).max(1.0));
    }
    let n = ((t1 - t0) / h).ceil().clamp(1.0, 4096.0) as usize;
    let step = (t1 - t0) / n as f64;
    let at = |t: f64| conic.state_at(t);
    let radial = |t: f64| {
        let (r, v) = at(t);
        r.dot(v)
    };
    let dist = |t: f64| at(t).0.length();
    let moon_dist = |c: usize, t: f64| {
        let (r, _) = bodies[c].relative(b.mu, t);
        (at(t).0 - r).length()
    };
    let mut a = t0;
    let mut radial_a = radial(a);
    for i in 1..=n {
        let e = if i == n { t1 } else { t0 + step * i as f64 };
        let radial_e = radial(e);
        let mut found: Option<(f64, Event)> = None;
        let mut take = |t: f64, ev: Event| {
            if found.as_ref().is_none_or(|(f, _)| t < *f) {
                found = Some((t, ev));
            }
        };
        if floor {
            // Lowest point of this step: its periapsis, if it passes one.
            let low = if radial_a < 0.0 && radial_e >= 0.0 {
                bisect(a, e, |t| radial(t) >= 0.0)
            } else {
                e
            };
            if dist(low) < b.floor {
                take(bisect(a, low, |t| dist(t) < b.floor), Event::Floor);
            }
        }
        if leave {
            let high = if radial_a > 0.0 && radial_e <= 0.0 {
                bisect(a, e, |t| radial(t) <= 0.0)
            } else {
                e
            };
            if dist(high) > b.soi {
                take(bisect(a, high, |t| dist(t) > b.soi), Event::Leave);
            }
        }
        for &c in &moons {
            let edge = bodies[c].soi * ENTER;
            if moon_dist(c, e) < edge {
                take(bisect(a, e, |t| moon_dist(c, t) < edge), Event::Enter(c));
            }
        }
        if found.is_some() {
            return found;
        }
        a = e;
        radial_a = radial_e;
    }
    None
}

/// The longest step (space seconds) a burn is applied in; a frame at a high warp is split
/// into up to [`MAX_BURN_STEPS`] of them.
const BURN_STEP_S: f64 = 2.0;
const MAX_BURN_STEPS: usize = 64;

/// Flies `conic` around body `p` from `from` to `to`, handing it from one sphere of influence
/// to the next. Returns the time it reached the floor (air or low space), if it did.
fn coast(
    conic: &mut Conic,
    p: &mut usize,
    table: &[Body],
    from: f64,
    to: f64,
    log: &mut RailsLog,
) -> Option<f64> {
    let mut from = from;
    for _ in 0..8 {
        match next_event(conic, *p, table, from, to) {
            None => break,
            Some((at, Event::Floor)) => return Some(at),
            Some((at, Event::Leave)) => {
                let Some(parent) = table[*p].parent else {
                    break;
                };
                let (r, v) = conic.state_at(at);
                let (pr, pv) = table[*p].relative(table[parent].mu, at);
                *conic = Conic::new(r + pr, v + pv, table[parent].mu, at);
                *p = parent;
                from = at;
                log.handoffs += 1;
            }
            Some((at, Event::Enter(c))) => {
                let (r, v) = conic.state_at(at);
                let (cr, cv) = table[c].relative(table[*p].mu, at);
                *conic = Conic::new(r - cr, v - cv, table[c].mu, at);
                *p = c;
                from = at;
                log.handoffs += 1;
            }
        }
    }
    None
}

/// After the space module has placed the bodies (`PostUpdate`): fly coasting ships on rails
/// while the clock runs fast, put free bodies out in space on rails, and hand them back to
/// physics when it slows to real time.
pub fn fly(world: &mut World) {
    let clock = *world.resource::<SpaceClock>();
    let settings = world.resource::<SpaceSettings>().clone();
    let t = clock.seconds;
    let dt = world.resource::<Time>().delta_secs_f64();
    let mut log = world.resource::<RailsLog>().clone();
    let last = log.last_clock.replace(t).unwrap_or(t);
    // Space seconds per world second. The clock's own warp unless it follows the weather's
    // time, whose rate is measured (smoothed: the clock ticks in fixed steps).
    log.rate = if clock.paused {
        0.0
    } else if settings.follow_world_clock {
        let seen = if dt > 0.0 { (t - last) / dt } else { 1.0 };
        let k = (dt / 0.5).clamp(0.0, 1.0);
        log.rate + (seen - log.rate) * k
    } else {
        clock.warp
    };
    let fast = log.rate > settings.rails_above_warp;

    let mut on_rails = world.query::<(Entity, &OnRails)>();
    let railed: Vec<(Entity, OnRails)> =
        on_rails.iter(world).map(|(e, r)| (e, r.clone())).collect();
    if !fast {
        // Real time again: physics carries on from where the rails left each ship.
        for (e, _) in railed {
            world.entity_mut(e).remove::<OnRails>();
        }
        *world.resource_mut::<RailsLog>() = log;
        return;
    }

    let bodies = collect_bodies(world);
    if bodies.is_empty() {
        *world.resource_mut::<RailsLog>() = log;
        return;
    }
    let frame = world.resource::<SpaceFrame>().clone();
    let states = propagate(&bodies, t);
    let table = table(&bodies, &states);
    let anchor = anchor_index(&states, &frame);
    let map = frame_for(&states[anchor], &frame);
    let here = FrameRef {
        anchor: &states[anchor],
        rotating: frame.rotating,
        map,
    };

    // Free bodies out in space go on rails, from where physics had them last frame.
    let parked: HashSet<Entity> = world
        .resource::<Parked>()
        .bodies
        .values()
        .flatten()
        .map(|p| p.entity)
        .collect();
    let mut free = world.query_filtered::<(Entity, &Transform, &Velocity, &RigidBody), (
        Without<ChildOf>,
        Without<CelestialBody>,
        Without<NoRails>,
        Without<OnRails>,
    )>();
    let candidates: Vec<(Entity, Transform, Velocity)> = free
        .iter(world)
        .filter(|(e, _, _, rb)| rb.kind == Motion::Dynamic && !parked.contains(e))
        .map(|(e, tf, v, _)| (e, *tf, *v))
        .collect();
    let mut flights = railed;
    if !candidates.is_empty() {
        let then = if last != t {
            propagate(&bodies, last)
        } else {
            states.clone()
        };
        let then_anchor = anchor_index(&then, &frame);
        let then_map = frame_for(&then[then_anchor], &frame);
        let was = FrameRef {
            anchor: &then[then_anchor],
            rotating: frame.rotating,
            map: then_map,
        };
        let probe = SpaceState {
            bodies: then.clone(),
            ..Default::default()
        };
        for (e, tf, v) in candidates {
            let r = then_map.0.inverse() * (tf.translation - then_map.1);
            let vel = then_map.0.inverse() * v.linear.as_dvec3() + was.velocity_at(r);
            let Some(p) = probe.dominant(r) else {
                continue;
            };
            let rel = r - then[p].inertial_position;
            if rel.length() < table[p].floor {
                continue;
            }
            let rails = OnRails {
                around: table[p].name.clone(),
                conic: Conic::new(rel, vel - then[p].inertial_velocity, table[p].mu, last),
                attitude: (then_map.0.inverse() * tf.rotation.as_dquat()).normalize(),
                checked: last,
                placed: (then_map.0, tf.rotation),
            };
            flights.push((e, rails));
        }
    }

    for (e, mut rails) in flights {
        let Some(mut p) = table.iter().position(|b| b.name == rails.around) else {
            world.entity_mut(e).remove::<OnRails>();
            continue;
        };
        let mut conic = rails.conic;
        let entity = world.entity(e);
        // The game turned the ship since it was placed: hold the new attitude.
        if let Some(tf) = entity.get::<Transform>()
            && tf.rotation.angle_between(rails.placed.1) > 1e-5
        {
            rails.attitude = (rails.placed.0.inverse() * tf.rotation.as_dquat()).normalize();
        }
        // Its engines: thrust over mass, turned from the world's axes into inertial ones.
        let mass = entity
            .get::<MassProperties>()
            .map(|m| m.mass_kg)
            .filter(|m| *m > 0.0)
            .or_else(|| entity.get::<RigidBody>().and_then(|rb| rb.mass_kg));
        let accel = entity
            .get::<ExternalForce>()
            .zip(mass)
            .map(|(f, m)| map.0.inverse() * (f.force / m).as_dvec3())
            .filter(|a| a.length_squared() > 0.0);
        let landed = match accel {
            None => coast(&mut conic, &mut p, &table, rails.checked, t, &mut log),
            Some(a) => {
                // Burn in short steps: coast each, then add its share of the push.
                let span = t - rails.checked;
                let steps = ((span / BURN_STEP_S).ceil() as usize).clamp(1, MAX_BURN_STEPS);
                let mut landed = None;
                for i in 0..steps {
                    let a0 = rails.checked + span * i as f64 / steps as f64;
                    let a1 = rails.checked + span * (i + 1) as f64 / steps as f64;
                    landed = coast(&mut conic, &mut p, &table, a0, a1, &mut log);
                    if landed.is_some() {
                        break;
                    }
                    let (r, v) = conic.state_at(a1);
                    conic = Conic::new(r, v + a * (a1 - a0), conic.mu, a1);
                }
                landed
            }
        };
        // Where it is now, around where its body is now.
        let (r, v) = conic.state_at(landed.unwrap_or(t));
        let inertial = states[p].inertial_position + r;
        let velocity = states[p].inertial_velocity + v;
        let position = map.0 * inertial + map.1;
        let linear = map.0 * (velocity - here.velocity_at(inertial));
        let mut entity = world.entity_mut(e);
        let rotation = (map.0 * rails.attitude).normalize().as_quat();
        if let Some(mut tf) = entity.get_mut::<Transform>() {
            tf.translation = position;
            tf.rotation = rotation;
        }
        if let Some(mut vel) = entity.get_mut::<Velocity>() {
            vel.linear = linear.as_vec3();
            vel.angular = Vec3::ZERO;
        }
        if let Some(mut i) = entity.get_mut::<TransformInterpolation>() {
            i.reset();
        }
        if landed.is_some() {
            entity.remove::<OnRails>();
            log.warp_drops += 1;
            if !settings.follow_world_clock {
                world.resource_mut::<SpaceClock>().warp = 1.0;
            }
        } else {
            rails.around = table[p].name.clone();
            rails.conic = Conic::new(r, v, conic.mu, t);
            rails.checked = t;
            rails.placed = (map.0, rotation);
            entity.insert(rails);
        }
    }
    *world.resource_mut::<RailsLog>() = log;
}
