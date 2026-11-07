//! `spells_shield` (round-11-plan.md, "The hall stop"): shields held against two battle mages.
//!
//! Two battle mages in red stand on the floor before the dais, each facing a holder on a
//! plinth (lanes 3 and 4, counted from 0):
//!
//! * Lane 4 (Reinhardt's barrier): raises a plain shield and holds it under a barrage of fire
//!   bolts. It flares where each lands, cracks at half and a quarter, flickers, and shatters,
//!   and the holder staggers. Locked out, its next try only sputters; after the lockout it
//!   raises it again, takes four bolts, and once they stop the shield regrows whole.
//! * Lane 3 (a perfect Protego): raises its shield just as each bolt arrives, and the bolt flies
//!   back and bursts on the mage who cast it, a fire bolt and then a frost bolt; then it raises
//!   too early, and the bolt only bursts on the shield. In the second half it holds a fire
//!   shield into a flame jet, which it soaks for nothing (white flares), then frost bolts cost it
//!   double.
//!
//! The battle mages are the program's own ([`ShieldFoe`], cast by [`run_foes`] on the hall's
//! clock); the holders are the hall's lane mages.

use engine_core::math::{DVec3, Quat};
use engine_core::prelude::*;

use super::art::*;
use super::{AimAt, Cast, Hall, Lane, Program, d, lane};
use crate::caster::{Aim, CastRequest, MagicCaster, SpellChoice, Vitality};
use crate::design::{CastType, Modifier, SpellDesign};
use crate::schools::Mastery;

/// When the program starts in its period, s (the last program's smoke settles first).
pub const START: f32 = 1.2;
/// The battle mages' side.
pub const FOES: u32 = 2;
/// The holders' lanes.
pub const HOLDERS: [usize; 2] = [3, 4];
/// Where each battle mage stands (x, z from the hall's centre), facing its holder.
pub const FOE_AT: [[f64; 2]; 2] = [[4.6, 2.2], [7.6, 2.2]];
/// The battle mages' level and mastery: their bolts are a bit weaker than the holders'.
pub const FOE_LEVEL: f32 = 50.0;
pub const FOE_MASTERY: Mastery = Mastery::Ascended1;
/// The tag the battle mages wear as `HallMage::lane` (beyond the lanes, so the lane programs
/// pass them by, as the duel's masters do).
pub const FOE_TAG: usize = 110;

/// A battle mage of `spells_shield`.
#[derive(Component, Debug, Clone, Copy)]
pub struct ShieldFoe {
    pub index: usize,
}

/// Lane 3's perfect blocks: when its battle mage casts each bolt (from [`START`]) and how long
/// before the bolt reaches it the holder raises its shield (0.1 s: inside the 0.25 s window).
pub const PERFECT_BOLTS: [f32; 2] = [0.6, 3.0];
/// The bolt it raises too early for, and how early.
pub const EARLY_BOLT: f32 = 5.4;
pub const EARLY_BY: f32 = 0.7;
/// The shield is up this long before the bolt meets it, for a perfect block.
pub const PERFECT_LEAD: f32 = 0.1;

/// Seconds from a bolt cast by battle mage `foe` to its meeting holder `lane`'s shield, and from
/// a shield cast by that holder to its standing: from the spells' gathering times and the bolt's
/// flight (the hall steps at 1/60 s; a shield stands the step after it is raised).
pub fn timings(foe: usize, lane: usize) -> (f32, f32) {
    let foe_bolt = crate::compile::compile(
        &fire_bolt(),
        crate::compile::CastContext::new(FOE_MASTERY, crate::schools::Power::at_level(FOE_LEVEL)),
    )
    .expect("a fire bolt compiles");
    let shield = crate::compile::compile(
        &plain_shield(0.25),
        crate::compile::CastContext::new(Mastery::Ascended1, crate::schools::Power::at_level(62.0)),
    )
    .expect("a shield compiles");
    let hand = foe_hand(foe);
    let holder = DVec3::new(LANES[lane], PLINTH_TOP, MAGE_Z);
    let (face, _) =
        crate::wards::shield_pose(holder, 1.8, Some(hand), engine_core::math::Vec3::NEG_Z);
    let flight = ((face - hand).length() as f32 - 0.1) / foe_bolt.carrier.speed;
    (foe_bolt.cast_secs + flight, shield.cast_secs + 1.0 / 60.0)
}

/// Where battle mage `foe`'s hand is (it faces the dais).
pub fn foe_hand(foe: usize) -> DVec3 {
    let [x, z] = FOE_AT[foe];
    let hand = Quat::from_rotation_y(std::f32::consts::PI) * MagicCaster::default().hand;
    DVec3::new(x, FLOOR, z) + hand.as_dvec3()
}

fn fire_bolt() -> SpellDesign {
    d(&["fire"], CastType::Bolt).named("Fire bolt")
}

fn frost_bolt() -> SpellDesign {
    d(&["frost"], CastType::Bolt).named("Frost bolt")
}

/// A plain shield held for `hold` × 4 s.
pub fn plain_shield(hold: f32) -> SpellDesign {
    d(&["ward"], CastType::Cone)
        .with(Modifier::Duration { factor: hold })
        .named("Shield")
}

/// What each battle mage casts (at, from [`START`]) and the design.
pub fn foe_casts(foe: usize) -> Vec<(f32, SpellDesign)> {
    match foe {
        // At lane 3: two bolts for perfect blocks, one for an early raise, then a flame jet on
        // the fire shield and frost bolts.
        0 => vec![
            (PERFECT_BOLTS[0], fire_bolt()),
            (PERFECT_BOLTS[1], frost_bolt()),
            (EARLY_BOLT, fire_bolt()),
            (
                8.6,
                d(&["fire"], CastType::Beam)
                    .with(Modifier::Duration { factor: 1.0 })
                    .named("Flame jet"),
            ),
            (11.8, frost_bolt()),
            (12.3, frost_bolt()),
            (12.8, frost_bolt()),
        ],
        // At lane 4: a barrage until the shield breaks, then four bolts at the second one.
        _ => {
            let mut v: Vec<(f32, SpellDesign)> = (0..BARRAGE)
                .map(|i| (0.9 + 0.42 * i as f32, fire_bolt()))
                .collect();
            v.extend((0..4).map(|i| (SECOND_BOLTS + 0.45 * i as f32, fire_bolt())));
            v
        }
    }
}

/// Bolts in lane 4's first barrage.
pub const BARRAGE: usize = 9;
/// Lane 4 raises its shield again (after the lockout), and the battle mage's four bolts at it
/// start.
pub const SECOND_RAISE: f32 = 8.0;
pub const SECOND_HOLD: f32 = 1.5;
pub const SECOND_BOLTS: f32 = 8.6;

/// The program's lanes: the holders' shields.
pub(super) fn lanes() -> Vec<Lane> {
    let cast = |at: f32, design: SpellDesign, aim: AimAt| Cast {
        at: at + START,
        design: Some(design),
        aim,
    };
    let casts = |casts: Vec<Cast>| Lane {
        casts,
        ..lane(SpellDesign::default())
    };
    let foe = |i: usize| AimAt::Mage(FOE_TAG + i);
    // Lane 3 raises its shield so it stands PERFECT_LEAD before each bolt meets it.
    let (bolt, up) = timings(0, 3);
    let stagger = 0.13 * 3.0;
    let raise_for = |bolt_at: f32, lead: f32| bolt_at + bolt - up - lead - stagger;
    let mut v: Vec<Lane> = (0..LANES.len()).map(|_| casts(Vec::new())).collect();
    v[3] = casts(vec![
        cast(
            raise_for(PERFECT_BOLTS[0], PERFECT_LEAD),
            plain_shield(0.25),
            foe(0),
        ),
        cast(
            raise_for(PERFECT_BOLTS[1], PERFECT_LEAD),
            plain_shield(0.25),
            foe(0),
        ),
        cast(raise_for(EARLY_BOLT, EARLY_BY), plain_shield(0.3), foe(0)),
        cast(
            8.0,
            d(&["ward", "fire"], CastType::Cone)
                .with(Modifier::Duration { factor: 1.4 })
                .named("Fire shield"),
            foe(0),
        ),
    ]);
    v[4] = casts(vec![
        // Held until it breaks under the barrage.
        cast(0.2, plain_shield(1.6), foe(1)),
        // Locked out: it sputters.
        cast(5.6, plain_shield(1.0), foe(1)),
        // Raised again: four bolts, then it regrows.
        cast(SECOND_RAISE, plain_shield(SECOND_HOLD), foe(1)),
    ]);
    v
}

/// The battle mages, facing the dais.
pub fn spawn_props(commands: &mut Commands, origin: DVec3) {
    for (i, [x, z]) in FOE_AT.iter().enumerate() {
        let at = origin + DVec3::new(*x, FLOOR, *z);
        let e = spawn_mage(commands, at, FOE_TAG + i, FOE_MASTERY);
        commands.entity(e).insert((
            Name::new(format!("Battle mage {}", i + 1)),
            Transform::from_translation(at)
                .with_rotation(Quat::from_rotation_y(std::f32::consts::PI)),
            MagicCaster {
                learns: false,
                side: FOES,
                seed: 6100 + i as u64,
                max_mana: 1.0e6,
                mana: 1.0e6,
                mana_regen: 1.0e5,
                ..MagicCaster::new(FOE_LEVEL, FOE_MASTERY)
            },
            mage_look(110 + i, FOE_MASTERY, Some([0.85, 0.1, 0.06])),
            ProgramProp,
            ShieldFoe { index: i },
            Vitality {
                regen: 400.0,
                ..Vitality::new(4000.0)
            },
        ));
    }
}

/// Cast the battle mages' spells on the hall's clock while `spells_shield` runs.
pub fn run_foes(
    hall: Option<Res<Hall>>,
    foes: Query<(Entity, &ShieldFoe)>,
    mages: Query<(Entity, &HallMage)>,
    mut last: Local<Option<f32>>,
    mut casts: MessageWriter<CastRequest>,
) {
    let Some(hall) = hall else {
        return;
    };
    let t1 = hall.t;
    let t0 = last.replace(t1).unwrap_or(t1);
    if hall.program != Program::Shield || !hall.near || hall.restart || t1 < t0 {
        return;
    }
    let period = Program::Shield.period();
    for (e, foe) in &foes {
        let Some(holder) = HOLDERS.get(foe.index) else {
            continue;
        };
        let Some((target, _)) = mages.iter().find(|(_, m)| m.lane == *holder) else {
            continue;
        };
        for (at, design) in foe_casts(foe.index) {
            let at = at + START;
            let (pa, pb) = (t0 - at, t1 - at);
            let crossed = pb >= 0.0 && (pa < 0.0 || (pa / period).floor() != (pb / period).floor());
            if crossed {
                casts.write(CastRequest {
                    caster: e,
                    spell: SpellChoice::Design(design),
                    aim: Aim::Entity(target),
                    charge: 0.0,
                    gathered: false,
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::{CastContext, compile};
    use crate::schools::Power;

    #[test]
    fn lane_3_s_shield_stands_inside_the_window_when_each_bolt_arrives() {
        let (bolt, up) = timings(0, 3);
        assert!(up < 0.2, "a shield comes up fast: {up}");
        for b in PERFECT_BOLTS {
            let raised = b + bolt - up - PERFECT_LEAD;
            let stands = raised + up;
            let meets = b + bolt;
            let into = meets - stands;
            assert!(
                (0.03..crate::wards::PERFECT_SECS - 0.05).contains(&into),
                "meets {into} s after it stands"
            );
        }
        // Raised early, the window is shut when the bolt arrives.
        const { assert!(EARLY_BY > crate::wards::PERFECT_SECS + 0.2) };
        // Every raise is in the period, after the start.
        for l in lanes() {
            for c in l.casts {
                assert!(c.at > START && c.at < START + 15.0, "{}", c.at);
            }
        }
    }

    #[test]
    fn lane_4_s_shield_breaks_late_in_the_barrage_and_regrows_after_the_second() {
        let shield = compile(
            &plain_shield(1.6),
            CastContext::new(Mastery::Ascended1, Power::at_level(62.0)),
        )
        .unwrap();
        let bolt = compile(
            &fire_bolt(),
            CastContext::new(FOE_MASTERY, Power::at_level(FOE_LEVEL)),
        )
        .unwrap();
        let pool = shield.payload.ward * crate::wards::SHIELD_POOL;
        let hits = pool / bolt.payload.damage;
        println!(
            "pool {pool}, bolt {}, {hits} bolts, holds {} s",
            bolt.payload.damage, shield.payload.shield_secs
        );
        assert!(
            (BARRAGE as f32 - 2.5..BARRAGE as f32).contains(&hits),
            "breaks after {hits} of {BARRAGE} bolts"
        );
        // Four bolts take it past a crack, not to breaking.
        let left = 1.0 - 4.0 / hits;
        assert!((0.25..0.5).contains(&left), "{left}");
        // Held long enough to break; the second knits its crack back while it is still up, and
        // is down before the period ends.
        assert!(shield.payload.shield_secs > 0.9 + 0.42 * BARRAGE as f32);
        let (bolt_secs, _) = timings(1, 4);
        let stagger = 0.13 * 4.0;
        let second = compile(
            &plain_shield(SECOND_HOLD),
            CastContext::new(Mastery::Ascended1, Power::at_level(62.0)),
        )
        .unwrap();
        let down = SECOND_RAISE + stagger + second.payload.shield_secs;
        let last_hit = SECOND_BOLTS + 0.45 * 3.0 + bolt_secs;
        let knits =
            last_hit + crate::wards::REGROW_AFTER + (0.5 - left) / crate::wards::REGROW_RATE;
        assert!(knits + 1.0 < down, "knits at {knits}, down at {down}");
        assert!(START + down < Program::Shield.period(), "down at {down}");
        // The first breaks, and the lockout ends, between the sputter and the second raise.
        let breaks = 0.9 + 0.42 * (hits.ceil() - 1.0) + bolt_secs;
        let unlocked = breaks + crate::wards::LOCK_SECS;
        assert!(breaks < 5.6 + stagger && 5.6 + stagger < unlocked);
        assert!(unlocked < SECOND_RAISE + stagger, "unlocked at {unlocked}");
    }
}
