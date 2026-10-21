//! Frag grenades. A smart squad lobs one at an enemy that stays put behind cover while it is
//! pinned (see `brain`, which also picks a smoke canister instead when a mate is near the
//! spot): it flies in an arc, lands, and goes off a moment later through the same blast as an
//! explosive prop (`features`): a `WorldImpulse` (wildlife, smoke, foliage and destruction
//! react to it), a `SmokeEmission`, a `NoiseEvent` everyone hears and `DamageEvent`s to what
//! the blast reaches past cover. No fire: a frag isn't a fuel tank.
//!
//! Game code can see what is coming through [`Grenades`]: [`Grenades::incoming`] says whether
//! one is due to land near a spot (a player warning, or a scripted dive in a test).

use engine_core::ecs::message::Messages;
use engine_core::prelude::*;
use engine_render::{MeshRenderer, Shape, StandardMaterial};
use engine_world::{DamageEvent, NoiseEvent, SmokeEmission, WorldImpulse};

use crate::features::{Blast, blast};
use crate::perception::Perceivables;
use crate::world_model::TacticalWorld;

/// Blast radius (m): the damage falls off to nothing there.
pub const FRAG_RADIUS: f32 = 6.0;
/// Damage at the center.
pub const FRAG_DAMAGE: f32 = 140.0;
/// From landing to going off (s): a roll and the rest of the fuse.
pub const FUSE_SECS: f64 = 0.8;

/// A grenade in the air or on the ground.
#[derive(Debug, Clone, Copy)]
pub struct Flight {
    pub id: u32,
    /// From the thrower's hand to where it lands (world).
    pub from: DVec3,
    pub at: DVec3,
    /// When it was thrown (world clock, s), and how long it flies.
    pub thrown: f64,
    pub flight_secs: f64,
    pub thrower: Entity,
    /// Who it was thrown at.
    pub target: Option<Entity>,
}

impl Flight {
    /// When it goes off (world clock, s).
    pub fn goes_off(&self) -> f64 {
        self.thrown + self.flight_secs + FUSE_SECS
    }

    /// Where it is `now`: on a lob from the hand to the spot (a little over a third as high
    /// as it is long, at least 2.5 m over the hand), then on the ground.
    pub fn position(&self, now: f64) -> DVec3 {
        let k = ((now - self.thrown) / self.flight_secs).clamp(0.0, 1.0);
        let len = DVec2::new(self.at.x - self.from.x, self.at.z - self.from.z).length();
        let apex = (len * 0.35).max(2.5);
        let p = self.from.lerp(self.at, k);
        p + DVec3::Y * (4.0 * apex * k * (1.0 - k))
    }
}

/// The grenades thrown and not yet gone off, and which enemies had one lately.
#[derive(Resource, Debug, Clone, Default)]
pub struct Grenades {
    pub flights: Vec<Flight>,
    /// (target, when thrown) for the last while: a squad throws one at a time at an enemy.
    pub recent: Vec<(Entity, f64)>,
    next: u32,
}

impl Grenades {
    /// Throw one from `from` to land at `at`.
    pub fn throw(
        &mut self,
        from: DVec3,
        at: DVec3,
        now: f64,
        thrower: Entity,
        target: Option<Entity>,
    ) {
        let len = from.distance(at);
        self.flights.push(Flight {
            id: self.next,
            from,
            at,
            thrown: now,
            flight_secs: 0.6 + len / 22.0,
            thrower,
            target,
        });
        self.next = self.next.wrapping_add(1);
        self.thrown_at(target, now);
    }

    /// Note a throw (a frag or smoke) at `target`.
    pub fn thrown_at(&mut self, target: Option<Entity>, now: f64) {
        if let Some(t) = target {
            self.recent.push((t, now));
        }
    }

    /// Whether anything was thrown at `target` in the last `secs`.
    pub fn lately(&self, target: Entity, now: f64, secs: f64) -> bool {
        self.recent
            .iter()
            .any(|&(t, when)| t == target && now - when < secs)
    }

    /// Whether a grenade is due to go off within `radius` m of `p`.
    pub fn incoming(&self, p: DVec3, radius: f64) -> bool {
        self.flights
            .iter()
            .any(|f| DVec2::new(f.at.x - p.x, f.at.z - p.z).length() < radius)
    }
}

/// Sets off the grenades whose fuse has run out.
#[allow(clippy::too_many_arguments)]
pub(crate) fn fly_grenades(
    mut grenades: ResMut<Grenades>,
    time: Res<Time>,
    seen: Res<Perceivables>,
    world: Res<TacticalWorld>,
    mut impulses: MessageWriter<WorldImpulse>,
    mut smoke: MessageWriter<SmokeEmission>,
    mut noise: MessageWriter<NoiseEvent>,
    mut damage: ResMut<Messages<DamageEvent>>,
    mut near: Local<Vec<u32>>,
) {
    let now = time.elapsed_secs_f64();
    grenades.recent.retain(|&(_, when)| now - when < 60.0);
    if grenades.flights.is_empty() {
        return;
    }
    let mut k = 0;
    while k < grenades.flights.len() {
        let f = grenades.flights[k];
        if now < f.goes_off() {
            k += 1;
            continue;
        }
        grenades.flights.swap_remove(k);
        let b = Blast {
            center: f.at + DVec3::Y * 0.3,
            radius: FRAG_RADIUS,
            damage: FRAG_DAMAGE,
            source: Some(f.thrower),
            heat_c: 400.0,
            energy_j: 120_000.0,
            strength: 24.0,
        };
        blast(
            &b,
            &seen,
            Some(&world),
            &mut near,
            &mut impulses,
            None,
            &mut smoke,
            &mut noise,
            &mut damage,
        );
    }
}

/// A grenade's body while it flies and lies there.
#[derive(Component, Debug, Clone, Copy)]
pub struct GrenadeBody {
    pub id: u32,
}

/// Draws the grenades: a small dark egg along its arc.
pub(crate) fn show_grenades(
    mut commands: Commands,
    grenades: Res<Grenades>,
    mut bodies: Query<(Entity, &GrenadeBody, &mut Transform)>,
    time: Res<Time>,
) {
    let now = time.elapsed_secs_f64();
    let mut have = Vec::new();
    for (e, b, mut t) in &mut bodies {
        match grenades.flights.iter().find(|f| f.id == b.id) {
            Some(f) => {
                t.translation = f.position(now) + DVec3::Y * 0.05;
                have.push(f.id);
            }
            None => commands.entity(e).despawn(),
        }
    }
    for f in &grenades.flights {
        if have.contains(&f.id) {
            continue;
        }
        commands.spawn((
            GrenadeBody { id: f.id },
            Transform::from_translation(f.position(now)).with_scale(Vec3::new(1.0, 1.3, 1.0)),
            MeshRenderer::new(
                Shape::Sphere {
                    radius: 0.05,
                    segments: 10,
                },
                StandardMaterial::hex(0x3d4630).with_roughness(0.7),
            ),
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_grenade_arcs_over_and_lands_where_it_was_thrown() {
        let mut g = Grenades::default();
        let from = DVec3::new(0.0, 1.6, 20.0);
        let at = DVec3::new(0.5, 0.0, 0.0);
        g.throw(from, at, 10.0, Entity::from_raw_u32(3).unwrap(), None);
        let f = g.flights[0];
        assert!(f.position(10.0).distance(from) < 1e-9);
        let mid = f.position(10.0 + f.flight_secs * 0.5);
        assert!(mid.y > 1.6 + 2.5, "the lob tops out at {:.1} m", mid.y);
        assert!(f.position(10.0 + f.flight_secs).distance(at) < 1e-9);
        assert!(f.goes_off() > 10.0 + f.flight_secs);
        assert!(g.incoming(DVec3::new(2.0, 0.0, 1.0), 3.0));
        assert!(!g.incoming(DVec3::new(6.0, 0.0, 1.0), 3.0));
    }
}
