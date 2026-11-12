//! Arrows lobbed over cover onto a hiding foe (`besiege`, [`crate::difficulty::Tactic::LobOver`]).
//!
//! A lob is aimed as an arc through an apex over the cover, as low as clears everything on
//! the way ([`aim`]), and flown a step at a time on the world clock: each step's piece of the
//! arc is sent as a `ProjectileTrace` (smoke parts, everyone near hears it whiz) and stops at
//! the first thing it meets, level geometry, the ground or a body, which it hurts
//! (`DamageKind::Pierce`). Game code reads [`Lobs`] for what is in the air and where the last
//! ones came down.

use engine_core::prelude::*;
use engine_world::physics::{PhysicsBackend, PhysicsQueries, QueryFilter};
use engine_world::{DamageEvent, DamageKind, Ground, NoiseEvent, ProjectileTrace};

use crate::agent::NpcCombat;
use crate::perception::Perceivables;
use crate::world_model::{Sight, TacticalWorld, physics_backend};

/// Gravity on an arrow (m/s²).
pub const GRAVITY: f64 = 9.81;
/// The apex: at least this far over the cover's top (m), and at most this much higher again.
const APEX_OVER: f64 = 1.5;
const APEX_MORE: f64 = 7.5;
/// The longest an arrow flies before it is dropped (s).
const MAX_FLIGHT: f64 = 6.0;
/// How close an arrow passes a body's middle to hit it (m).
const BODY_RADIUS: f32 = 0.3;
/// Landings kept in [`Lobs::landed`] this long (s).
const KEEP_LANDED: f64 = 30.0;

/// An arrow in the air.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Lob {
    pub from: DVec3,
    pub velocity: DVec3,
    /// When it was loosed (world clock, s), and how long it has flown so far.
    pub loosed: f64,
    pub flown: f64,
    pub shooter: Entity,
    pub target: Option<Entity>,
    pub damage: f32,
}

impl Lob {
    /// Where it is `t` seconds after it was loosed.
    pub fn position(&self, t: f64) -> DVec3 {
        self.from + self.velocity * t - DVec3::Y * (0.5 * GRAVITY * t * t)
    }
}

/// Lobbed arrows in the air and where the last ones came down.
#[derive(Resource, Debug, Default, Clone)]
pub struct Lobs {
    pub flights: Vec<Lob>,
    /// (world clock s, where it came down, whom it hit, who loosed it), newest last.
    pub landed: Vec<(f64, DVec3, Option<Entity>, Entity)>,
}

/// The velocity that lobs an arrow from `from` onto `to` over cover whose top is at `over`
/// (world height) at no more than `speed`: the lowest apex, from [`APEX_OVER`] over the top
/// up, whose arc `clear` finds unbroken (each piece of it; the last ends at `to`). `None`
/// when there is none.
pub fn aim(
    from: DVec3,
    to: DVec3,
    over: f64,
    speed: f32,
    clear: impl Fn(DVec3, DVec3) -> bool,
) -> Option<DVec3> {
    let flat = DVec2::new(to.x - from.x, to.z - from.z);
    let d = flat.length();
    if d < 1.0 {
        return None;
    }
    let dir = flat / d;
    let low = over.max(from.y).max(to.y) + APEX_OVER;
    let mut h = low;
    while h <= low + APEX_MORE {
        let up = (2.0 * GRAVITY * (h - from.y)).sqrt();
        let t = up / GRAVITY + (2.0 * (h - to.y) / GRAVITY).sqrt();
        let along = d / t;
        let v = DVec3::new(dir.x * along, up, dir.y * along);
        if v.length() <= speed as f64 {
            let lob = Lob {
                from,
                velocity: v,
                loosed: 0.0,
                flown: 0.0,
                shooter: Entity::PLACEHOLDER,
                target: None,
                damage: 0.0,
            };
            let n = 24;
            let mut a = from;
            let ok = (1..=n).all(|k| {
                let b = if k == n {
                    to
                } else {
                    lob.position(t * k as f64 / n as f64)
                };
                let fine = clear(a, b);
                a = b;
                fine
            });
            if ok {
                return Some(v);
            }
        }
        h += 0.75;
    }
    None
}

/// Fly the lobbed arrows a step on (Act, after shots are resolved).
#[allow(clippy::too_many_arguments)]
pub(crate) fn fly_lobs(
    mut lobs: ResMut<Lobs>,
    time: Res<Time>,
    seen: Res<Perceivables>,
    world: Res<TacticalWorld>,
    ground: Res<Ground>,
    physics: Res<PhysicsQueries>,
    backend: Option<Res<PhysicsBackend>>,
    mut fighters: Query<&mut NpcCombat>,
    mut traces: MessageWriter<ProjectileTrace>,
    mut damage: MessageWriter<DamageEvent>,
    mut noise: MessageWriter<NoiseEvent>,
    mut near: Local<Vec<u32>>,
) {
    let now = time.elapsed_secs_f64();
    lobs.landed.retain(|l| now - l.0 < KEEP_LANDED);
    if lobs.flights.is_empty() {
        return;
    }
    let phys = physics_backend(&backend, &physics);
    let lobs = &mut *lobs;
    let mut k = 0;
    while k < lobs.flights.len() {
        let f = &mut lobs.flights[k];
        let t0 = f.flown;
        let t1 = (now - f.loosed).min(MAX_FLIGHT);
        if t1 <= t0 {
            k += 1;
            continue;
        }
        f.flown = t1;
        let (a, b) = (f.position(t0), f.position(t1));
        let len = a.distance(b) as f32;
        let dir = ((b - a) / len.max(1e-6) as f64).as_vec3();
        // Level geometry.
        let mut stop = match phys {
            Some(p) => p
                .raycast(
                    a,
                    dir,
                    len,
                    &QueryFilter {
                        exclude: vec![f.shooter],
                        ..Default::default()
                    },
                )
                .map(|h| h.distance),
            None => world.raycast(a, b).map(|(t, _)| t),
        };
        // The ground.
        if let Some(g) = ground.height(b)
            && b.y <= g
        {
            let ga = ground.height(a).unwrap_or(g);
            let above = (a.y - ga).max(0.0);
            let t = len * (above / (above + (g - b.y)).max(1e-6)) as f32;
            stop = Some(stop.map_or(t, |s: f32| s.min(t)));
        }
        // Bodies.
        let mut hit = None;
        let reach = stop.unwrap_or(len);
        near.clear();
        seen.near(a.lerp(b, 0.5), len as f64 * 0.5 + 1.5, &mut near);
        for &i in near.iter() {
            let p = seen.list[i as usize];
            if p.entity == f.shooter {
                continue;
            }
            let lo = p.position + DVec3::Y * 0.2;
            let hi = p.position + DVec3::Y * (p.height as f64 - 0.1);
            let (t, gap) = crate::combat::ray_segment(a, dir, lo, hi);
            if gap < BODY_RADIUS && t <= reach && hit.is_none_or(|(_, ht)| t < ht) {
                hit = Some((p.entity, t));
            }
        }
        let end_t = hit.map(|h| h.1).or(stop);
        let end = end_t.map_or(b, |t| a + dir.as_dvec3() * t as f64);
        traces.write(ProjectileTrace {
            from: a,
            to: end,
            radius: 0.05,
            speed: (f.velocity - DVec3::Y * GRAVITY * t1).length() as f32,
            shooter: Some(f.shooter),
        });
        if end_t.is_none() && t1 < MAX_FLIGHT {
            k += 1;
            continue;
        }
        let f = lobs.flights.swap_remove(k);
        if let Some((victim, _)) = hit {
            damage.write(DamageEvent {
                target: victim,
                amount: f.damage,
                kind: DamageKind::Pierce,
                point: end,
                impulse: dir * f.damage * 2.0,
                source: Some(f.shooter),
            });
            if Some(victim) == f.target
                && let Ok(mut c) = fighters.get_mut(f.shooter)
            {
                c.stats.hits += 1;
            }
        }
        noise.write(NoiseEvent {
            position: end,
            radius: 12.0,
            source: Some(f.shooter),
        });
        lobs.landed.push((now, end, hit.map(|h| h.0), f.shooter));
    }
}

/// Whether a piece of a lob's arc is unbroken (`sight` with the shooter and its foe ignored).
pub(crate) fn arc_clear(sight: &Sight, ignore: &[Entity]) -> impl Fn(DVec3, DVec3) -> bool {
    move |a, b| sight.clear(a, b, ignore)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_arc_comes_down_where_it_was_aimed_and_clears_the_top() {
        let from = DVec3::new(0.0, 1.45, 18.0);
        let to = DVec3::new(0.0, 0.9, 0.0);
        let v = aim(from, to, 1.4, 60.0, |_, _| true).unwrap();
        let lob = Lob {
            from,
            velocity: v,
            loosed: 0.0,
            flown: 0.0,
            shooter: Entity::PLACEHOLDER,
            target: None,
            damage: 30.0,
        };
        // Its flight time, from its rise and fall.
        let t = (v.y + (v.y * v.y + 2.0 * GRAVITY * (from.y - to.y)).sqrt()) / GRAVITY;
        assert!(lob.position(t).distance(to) < 1e-6);
        let apex = v.y * v.y / (2.0 * GRAVITY) + from.y;
        assert!((apex - (from.y + APEX_OVER)).abs() < 1e-6, "{apex}");
        // A wall in the way of the lowest arc: a higher one.
        let wall = |a: DVec3, b: DVec3| !(a.z.min(b.z) < 8.0 && a.z.max(b.z) > 8.0 && a.y < 4.5);
        let high = aim(from, to, 1.4, 60.0, wall).unwrap();
        assert!(high.y > v.y);
        // Out of reach of a slow bow.
        assert!(aim(from, to, 1.4, 5.0, |_, _| true).is_none());
    }
}
