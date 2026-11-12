//! Bleeding over time: an open wound keeps costing health after its blow, as
//! `DamageKind::Bleed` damage (health only: nobody flinches, staggers or is alarmed by it), and
//! slowly clots. A cut-off limb bleeds hardest and clots slowest; a severed neck ends it.

use std::collections::HashMap;

use engine_core::prelude::*;
use engine_world::combat::{BodyInjuries, BodyZone};
use engine_world::{DamageEvent, DamageKind};

use crate::CombatSettings;
use crate::body::BodyCapsules;
use crate::resolve::to_world;

/// Least health a bleed tick carries: smaller losses add up until they reach it.
pub const BLEED_TICK_HP: f32 = 1.0;

/// Health lost per second by a body, from its open wounds' bleeding (0..1 each) at
/// `hp_per_s` for a fully bleeding part.
pub fn bleed_rate(injuries: &BodyInjuries, hp_per_s: f32) -> f32 {
    BodyZone::ALL
        .iter()
        .map(|&z| injuries.get(z).bleeding.clamp(0.0, 1.0))
        .sum::<f32>()
        * hp_per_s
}

/// Bleeding left after `dt` seconds of clotting with time constant `secs` (severed parts
/// clot three times slower).
pub fn clot(bleeding: f32, severed: bool, dt: f32, secs: f32) -> f32 {
    if secs <= 0.0 {
        return 0.0;
    }
    let tau = if severed { secs * 3.0 } else { secs };
    let left = bleeding * (-dt / tau).exp();
    if left < 0.01 { 0.0 } else { left }
}

/// Bleed every wounded body a little (Update, after the blows are judged): health goes in
/// `Bleed` ticks of at least [`BLEED_TICK_HP`], and each part's bleeding clots.
pub(crate) fn bleed(
    time: Res<Time>,
    settings: Res<CombatSettings>,
    mut bodies: Query<(Entity, &GlobalTransform, &mut BodyInjuries)>,
    mut owed: Local<HashMap<Entity, f32>>,
    mut damage: MessageWriter<DamageEvent>,
) {
    let dt = time.delta_secs();
    if dt <= 0.0 || !settings.rules {
        return;
    }
    owed.retain(|e, _| bodies.contains(*e));
    for (e, gt, mut inj) in &mut bodies {
        if inj.get(BodyZone::Neck).severed {
            owed.remove(&e);
            continue;
        }
        let rate = bleed_rate(&inj, settings.bleed_hp_per_s);
        if rate <= 0.0 {
            owed.remove(&e);
            continue;
        }
        // The part bleeding most is where the blood comes from.
        let worst = BodyZone::ALL
            .iter()
            .copied()
            .max_by(|a, b| inj.get(*a).bleeding.total_cmp(&inj.get(*b).bleeding))
            .unwrap_or(BodyZone::Chest);
        let o = owed.entry(e).or_default();
        *o += rate * dt;
        if *o >= BLEED_TICK_HP {
            damage.write(DamageEvent {
                target: e,
                amount: *o,
                kind: DamageKind::Bleed,
                point: to_world(
                    gt,
                    BodyCapsules::standing(1.75)
                        .center(worst)
                        .unwrap_or(Vec3::Y),
                ),
                impulse: Vec3::ZERO,
                source: None,
            });
            *o = 0.0;
        }
        // Clot (only bleeding bodies get here, so the change is real).
        for &z in BodyZone::ALL.iter() {
            let zi = inj.get_mut(z);
            if zi.bleeding > 0.0 {
                zi.bleeding = clot(zi.bleeding, zi.severed, dt, settings.bleed_clot_secs);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wounds_bleed_and_clot() {
        let mut inj = BodyInjuries::default();
        assert_eq!(bleed_rate(&inj, 2.0), 0.0);
        inj.get_mut(BodyZone::ThighR).bleeding = 0.5;
        inj.get_mut(BodyZone::ForearmL).bleeding = 1.0;
        assert!((bleed_rate(&inj, 2.0) - 3.0).abs() < 1e-5);
        // A cut clots within a minute or two; a stump keeps bleeding much longer.
        let mut cut = 0.5;
        let mut stump = 1.0;
        for _ in 0..600 {
            cut = clot(cut, false, 0.1, 25.0);
            stump = clot(stump, true, 0.1, 25.0);
        }
        assert!(cut < 0.05 && stump > 0.4, "cut {cut} stump {stump}");
        assert_eq!(clot(0.005, false, 0.1, 25.0), 0.0);
    }
}
