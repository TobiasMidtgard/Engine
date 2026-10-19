//! Engine exhausts: one `exhaust.*` effect per nozzle of every [`Exhausts`], matched by index,
//! following the nozzle (a child of its entity) and its throttle.
//!
//! The owner (vehicles, a jump kit) writes [`Exhausts`] each frame; this plays the effect for
//! each nozzle's kind, sized by its radius, glowing and spawning with its level. Jets and
//! tailpipes keep idling (haze, faint fumes) while the entity's [`EngineState`] says the
//! engine runs; everything else goes out at level 0, its smoke drifting on. The effects run
//! on their owner's [`LocalTimeScale`].

use std::collections::HashMap;

use engine_core::math::{Quat, Vec3};
use engine_core::prelude::*;
use engine_core::time::LocalTimeScale;
use engine_world::{EngineState, ExhaustKind, Exhausts};
use serde::{Deserialize, Serialize};

use crate::emitter::ParticleEffect;

/// The effect playing one nozzle of its parent's [`Exhausts`].
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ExhaustNozzle {
    /// Index into the parent's [`Exhausts`].
    pub index: usize,
}

/// The effect for a kind of exhaust, and the nozzle radius it is authored for at scale 1.
pub fn exhaust_effect(kind: ExhaustKind) -> (&'static str, f32) {
    match kind {
        ExhaustKind::Rocket => ("exhaust.rocket", 0.5),
        ExhaustKind::Jet => ("exhaust.jet", 0.5),
        ExhaustKind::Afterburner => ("exhaust.afterburner", 0.5),
        ExhaustKind::Thruster => ("exhaust.thruster", 0.5),
        ExhaustKind::Tailpipe => ("exhaust.tailpipe", 0.03),
        ExhaustKind::JumpJet => ("exhaust.jumpjet", 0.06),
    }
}

/// How an idling engine of this kind still shows (fraction of full), while it runs.
fn idle(kind: ExhaustKind) -> f32 {
    match kind {
        ExhaustKind::Jet | ExhaustKind::Tailpipe => 0.35,
        _ => 0.0,
    }
}

/// The level a nozzle shows at: its throttle, or its idle while the engine runs.
fn shown_level(n: &engine_world::Exhaust, engine: Option<&EngineState>) -> f32 {
    let running = engine.is_some_and(|e| e.running);
    n.level
        .clamp(0.0, 1.0)
        .max(if running { idle(n.kind) } else { 0.0 })
}

/// How densely a nozzle at `level` spawns. The effect keeps playing at 0, so its smoke drifts
/// on and it relights without starting over; its plume and glow are steady layers, which
/// only stop at 0 and otherwise keep their full rate.
fn rate_at(level: f32) -> f32 {
    if level > 0.01 { 0.4 + 0.6 * level } else { 0.0 }
}

type Owner<'a> = (
    Entity,
    &'a Exhausts,
    Option<&'a EngineState>,
    Option<&'a LocalTimeScale>,
);

type Nozzle<'a> = (
    Entity,
    &'a ExhaustNozzle,
    Option<&'a ChildOf>,
    &'a mut Transform,
    &'a mut ParticleEffect,
    Option<&'a LocalTimeScale>,
);

/// Spawn, update and remove the nozzle effects of every [`Exhausts`].
pub fn follow_exhausts(mut commands: Commands, owners: Query<Owner>, mut nozzles: Query<Nozzle>) {
    let mut have: HashMap<Entity, Vec<bool>> = HashMap::new();
    for (entity, nozzle, parent, mut tr, mut effect, scale) in &mut nozzles {
        // Orphans (the owner went in the frame its nozzle was spawned) go too.
        let Some(owner) = parent.map(ChildOf::parent) else {
            commands.entity(entity).despawn();
            continue;
        };
        let Some(ex) = owners
            .get(owner)
            .ok()
            .and_then(|(_, ex, engine, time)| ex.0.get(nozzle.index).map(|n| (*n, engine, time)))
        else {
            commands.entity(entity).despawn();
            continue;
        };
        let (n, engine, time) = ex;
        let slots = have.entry(owner).or_default();
        if slots.len() <= nozzle.index {
            slots.resize(nozzle.index + 1, false);
        }
        if slots[nozzle.index] {
            // A duplicate (two spawned for one nozzle in the same frame): keep one.
            commands.entity(entity).despawn();
            continue;
        }
        slots[nozzle.index] = true;
        let (name, radius) = exhaust_effect(n.kind);
        if effect.effect != name {
            effect.effect = name.to_string();
        }
        *tr = Transform::from_translation(n.position.as_dvec3()).with_rotation(
            Quat::from_rotation_arc(Vec3::Y, n.direction.normalize_or(Vec3::Z)),
        );
        let level = shown_level(&n, engine);
        effect.scale = (n.radius / radius).max(0.01);
        effect.intensity = level;
        effect.rate = rate_at(level);
        effect.playing = true;
        match (time, scale) {
            (Some(t), s) if s != Some(t) => {
                commands.entity(entity).insert(*t);
            }
            (None, Some(_)) => {
                commands.entity(entity).remove::<LocalTimeScale>();
            }
            _ => {}
        }
    }
    for (owner, ex, engine, time) in &owners {
        let slots = have.get(&owner);
        for (index, n) in ex.0.iter().enumerate() {
            if slots.and_then(|s| s.get(index)).copied().unwrap_or(false) {
                continue;
            }
            let (name, radius) = exhaust_effect(n.kind);
            let level = shown_level(n, engine);
            let mut effect = ParticleEffect::new(name)
                .with_scale((n.radius / radius).max(0.01))
                .with_intensity(level);
            effect.rate = rate_at(level);
            let mut nozzle = commands.spawn((
                Transform::from_translation(n.position.as_dvec3()).with_rotation(
                    Quat::from_rotation_arc(Vec3::Y, n.direction.normalize_or(Vec3::Z)),
                ),
                effect,
                ExhaustNozzle { index },
                ChildOf(owner),
            ));
            if let Some(t) = time {
                nozzle.insert(*t);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_world::Exhaust;

    fn app() -> App {
        let mut app = App::new();
        app.add_systems(Update, follow_exhausts);
        app
    }

    fn nozzles(app: &mut App) -> Vec<(ExhaustNozzle, ParticleEffect, Transform, Entity)> {
        let mut q = app
            .world
            .query::<(&ExhaustNozzle, &ParticleEffect, &Transform, &ChildOf)>();
        let mut v: Vec<_> = q
            .iter(&app.world)
            .map(|(n, e, t, p)| (*n, e.clone(), *t, p.parent()))
            .collect();
        v.sort_by_key(|(n, ..)| n.index);
        v
    }

    #[test]
    fn each_nozzle_gets_an_effect_that_follows_its_throttle() {
        let mut app = app();
        let ship = app
            .world
            .spawn((
                Transform::default(),
                Exhausts(vec![
                    Exhaust::new(ExhaustKind::Rocket, Vec3::new(0.0, 0.0, 4.0), Vec3::Z, 1.0)
                        .at_level(1.0),
                    Exhaust::new(ExhaustKind::Thruster, Vec3::X, Vec3::X, 0.1),
                ]),
            ))
            .id();
        app.update_n(2);
        let v = nozzles(&mut app);
        assert_eq!(v.len(), 2);
        let (_, rocket, at, parent) = &v[0];
        assert_eq!(*parent, ship);
        assert_eq!(rocket.effect, "exhaust.rocket");
        assert!(
            (rocket.scale - 2.0).abs() < 1e-5,
            "a 1 m radius is twice the 0.5 m nozzle"
        );
        assert!(rocket.playing && (rocket.intensity - 1.0).abs() < 1e-5);
        // The effect's +Y points down the exhaust.
        assert!((at.rotation * Vec3::Y).distance(Vec3::Z) < 1e-5);
        assert!(
            at.translation
                .distance(engine_core::math::DVec3::new(0.0, 0.0, 4.0))
                < 1e-9
        );
        let (_, thruster, ..) = &v[1];
        assert_eq!(thruster.effect, "exhaust.thruster");
        assert!(
            thruster.rate == 0.0 && thruster.intensity == 0.0,
            "an idle thruster is out"
        );
        assert!(
            thruster.playing,
            "but stays ready to relight where it left off"
        );

        // The throttle drops, a kind changes and a nozzle goes away.
        app.world.get_mut::<Exhausts>(ship).unwrap().0 = vec![
            Exhaust::new(
                ExhaustKind::Afterburner,
                Vec3::new(0.0, 0.0, 4.0),
                Vec3::Z,
                1.0,
            )
            .at_level(0.25),
        ];
        app.update_n(2);
        let v = nozzles(&mut app);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].1.effect, "exhaust.afterburner");
        assert!((v[0].1.intensity - 0.25).abs() < 1e-5);
    }

    #[test]
    fn running_jets_and_tailpipes_idle_and_stop_with_the_engine() {
        let mut app = app();
        let car = app
            .world
            .spawn((
                Transform::default(),
                Exhausts(vec![Exhaust::new(
                    ExhaustKind::Tailpipe,
                    Vec3::new(0.4, 0.3, 2.1),
                    Vec3::Z,
                    0.03,
                )]),
                EngineState {
                    running: true,
                    ..Default::default()
                },
            ))
            .id();
        app.update_n(2);
        let v = nozzles(&mut app);
        assert!(v[0].1.rate > 0.0, "an idling car puffs fumes");
        assert!((v[0].1.scale - 1.0).abs() < 1e-5);
        app.world.get_mut::<EngineState>(car).unwrap().running = false;
        app.update_n(1);
        assert_eq!(nozzles(&mut app)[0].1.rate, 0.0, "a parked car does not");
        // Without its exhausts the effect goes too.
        app.world.entity_mut(car).remove::<Exhausts>();
        app.update_n(2);
        assert!(nozzles(&mut app).is_empty());
    }

    #[test]
    fn nozzles_run_on_their_owners_time_and_orphans_go() {
        let mut app = app();
        let slow = LocalTimeScale {
            factor: 0.25,
            ignore_world_dilation: true,
        };
        let pack = app
            .world
            .spawn((
                Transform::default(),
                Exhausts(vec![Exhaust::new(
                    ExhaustKind::JumpJet,
                    Vec3::ZERO,
                    Vec3::NEG_Y,
                    0.06,
                )]),
                slow,
            ))
            .id();
        app.update_n(2);
        let nozzle = app
            .world
            .query_filtered::<Entity, With<ExhaustNozzle>>()
            .iter(&app.world)
            .next()
            .unwrap();
        assert_eq!(app.world.get::<LocalTimeScale>(nozzle), Some(&slow));
        app.world.entity_mut(pack).remove::<LocalTimeScale>();
        app.update_n(1);
        assert_eq!(app.world.get::<LocalTimeScale>(nozzle), None);

        // A nozzle whose owner vanished before it was parented is cleaned up.
        let orphan = app
            .world
            .spawn((
                Transform::default(),
                ParticleEffect::new("exhaust.jumpjet"),
                ExhaustNozzle { index: 0 },
            ))
            .id();
        app.update_n(1);
        assert!(app.world.get_entity(orphan).is_err());
    }
}
