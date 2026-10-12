//! Things in the world NPCs know how to use: explosive props they shoot when an enemy stands
//! next to one (and avoid standing next to themselves).
//!
//! When an [`ExplosiveProp`] is destroyed the module sends only world events: a
//! `WorldImpulse` (smoke, wind, foliage, destruction), an `IgniteRequest` (fire), a
//! `SmokeEmission`, a `NoiseEvent` (everyone hears it) and `DamageEvent`s to whatever is in the
//! blast. Whichever fire, smoke, destruction and audio modules are installed react to those.

use engine_core::ecs::message::{MessageCursor, Messages};
use engine_core::prelude::*;
use engine_render::MeshRenderer;
use engine_world::physics::Collider;
use engine_world::{
    DamageEvent, DamageKind, IgniteRequest, NoiseEvent, SmokeEmission, WorldImpulse,
};

use crate::perception::Perceivables;

/// A barrel, gas tank or crate that explodes when damaged enough.
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ExplosiveProp {
    /// Blast radius, meters.
    pub radius: f32,
    /// Damage at the center (falls off to 0 at the radius).
    pub damage: f32,
    /// Damage it takes before exploding.
    pub health: f32,
    /// Already went off.
    pub detonated: bool,
    /// Seconds until it is back (0 = never). Showcases use it.
    pub rearm_secs: f32,
    /// Seconds since it went off.
    #[serde(skip)]
    pub since: f32,
    #[serde(skip)]
    pub max_health: f32,
}

impl Default for ExplosiveProp {
    fn default() -> Self {
        Self {
            radius: 5.0,
            damage: 90.0,
            health: 20.0,
            detonated: false,
            rearm_secs: 0.0,
            since: 0.0,
            max_health: 20.0,
        }
    }
}

/// Collider and look of an explosive while it is intact (restored when it re-arms).
#[derive(Component, Debug, Clone)]
pub(crate) struct ExplosiveParts {
    collider: Option<Collider>,
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub(crate) fn explosives(
    mut commands: Commands,
    mut props: Query<(
        Entity,
        &Transform,
        &mut ExplosiveProp,
        Option<&Collider>,
        Option<&ExplosiveParts>,
    )>,
    mut cursor: Local<MessageCursor<DamageEvent>>,
    seen: Res<Perceivables>,
    time: Res<Time>,
    mut impulses: MessageWriter<WorldImpulse>,
    mut ignite: MessageWriter<IgniteRequest>,
    mut smoke: MessageWriter<SmokeEmission>,
    mut noise: MessageWriter<NoiseEvent>,
    mut damage: ResMut<Messages<DamageEvent>>,
    mut near: Local<Vec<u32>>,
) {
    let mut chain: Vec<(Entity, f32, Option<Entity>)> = cursor
        .read(&damage)
        .filter(|h| props.contains(h.target))
        .map(|h| (h.target, h.amount, h.source))
        .collect();
    let dt = time.delta_secs();
    // Re-arm.
    for (e, _, mut p, collider, parts) in &mut props {
        if p.max_health <= 0.0 {
            p.max_health = p.health;
        }
        if parts.is_none() {
            commands.entity(e).insert(ExplosiveParts {
                collider: collider.cloned(),
            });
        }
        if p.detonated && p.rearm_secs > 0.0 {
            p.since += dt;
            if p.since >= p.rearm_secs {
                p.detonated = false;
                p.since = 0.0;
                p.health = p.max_health;
                if let Some(c) = parts.and_then(|x| x.collider.clone()) {
                    commands.entity(e).insert(c);
                }
                commands.queue(move |world: &mut World| set_visible(world, e, true));
            }
        }
    }
    // Damage (blasts can chain to other barrels within the same tick).
    let mut i = 0;
    while i < chain.len() {
        let (target, amount, source) = chain[i];
        i += 1;
        let Ok((e, t, mut p, _, _)) = props.get_mut(target) else {
            continue;
        };
        if p.detonated {
            continue;
        }
        p.health -= amount;
        if p.health > 0.0 {
            continue;
        }
        p.detonated = true;
        p.since = 0.0;
        let (radius, blast) = (p.radius, p.damage);
        let origin = t.translation;
        let center = origin + DVec3::Y * 0.5;
        impulses.write(WorldImpulse {
            center,
            radius: radius * 2.5,
            strength: 30.0,
            energy_j: 250_000.0,
            heat_c: 900.0,
            source: source.or(Some(e)),
        });
        ignite.write(IgniteRequest {
            position: center,
            radius: radius * 0.6,
            temperature_c: 900.0,
            source: Some(e),
        });
        smoke.write(SmokeEmission {
            position: center,
            velocity: Vec3::Y * 8.0,
            radius: 1.5,
            mass_kg: 2.0,
            temperature_c: 600.0,
            darkness: 0.85,
        });
        noise.write(NoiseEvent {
            position: center,
            radius: 150.0,
            source: Some(e),
        });
        seen.near(center, radius as f64, &mut near);
        for &k in near.iter() {
            let victim = seen.list[k as usize];
            let d = (victim.position + DVec3::Y * 0.9).distance(center) as f32;
            if d > radius {
                continue;
            }
            let falloff = 1.0 - d / radius;
            let dir = ((victim.position - center).as_vec3() + Vec3::Y * 0.5).normalize_or(Vec3::Y);
            damage.write(DamageEvent {
                target: victim.entity,
                amount: blast * falloff,
                kind: DamageKind::Explosive,
                point: victim.position + DVec3::Y,
                impulse: dir * 400.0 * falloff,
                source: source.or(Some(e)),
            });
        }
        // Chain reactions.
        for (other, ot, op, _, _) in props.iter() {
            if other != e && !op.detonated && ot.translation.distance(origin) < radius as f64 {
                chain.push((other, blast, source));
            }
        }
        commands.entity(e).remove::<Collider>();
        commands.queue(move |world: &mut World| set_visible(world, e, false));
    }
}

/// Show or hide an entity and its children.
fn set_visible(world: &mut World, e: Entity, visible: bool) {
    let mut stack = vec![e];
    while let Some(x) = stack.pop() {
        if let Some(children) = world.get::<Children>(x) {
            stack.extend(children.iter());
        }
        if let Some(mut r) = world.get_mut::<MeshRenderer>(x) {
            r.visible = visible;
        }
    }
}
