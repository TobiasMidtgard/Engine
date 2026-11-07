//! `spells_cataclysm` (round-12-plan.md, "The hall stop"): rock from the sky and ground that
//! splits, down three lanes (counted from 0):
//!
//! * Lane 3 (an Expert's Meteor, fire and stone): a glowing ring on the floor round the loose
//!   dummy standing among crates and barrels halfway down the lane, pulsing faster as a burning
//!   boulder comes down out of the sky beyond it; it strikes, throws dummy, crates and barrels
//!   away and leaves a crater burning round the boulder, which cools from yellow to black.
//! * Lane 4 (a Master's Meteor swarm, plain stone, Split 5): five rings round the far dummies,
//!   five boulders one after another, five craters.
//! * Lane 2 (an Adept's Fault line): a crack runs from before the caster across the dais and
//!   down the lane through two loose dummies and a crate, heaving slabs up and throwing them;
//!   the aftershock bursts along it, then it grinds shut. Then a fire fault line, lava in the
//!   crack.

use engine_core::math::DVec3;
use engine_core::prelude::*;

use super::art::*;
use super::{AimAt, Cast, Lane, d, lane};
use crate::design::{CastType, Modifier, SpellDesign};
use crate::schools::Mastery;

/// When the program starts in its period, s (the last program's smoke settles first).
pub const START: f32 = 1.2;
/// Seconds between periods: every crater has sunk and every crack closed by its end.
pub const PERIOD: f32 = 22.0;
/// Lane 3's dummy among the crates (z from the hall's centre).
pub const METEOR_Z: f64 = -4.0;
/// Lane 2's loose dummies and crate on the fault's line (z).
pub const FAULT_DUMMIES: [f64; 2] = [2.5, -4.0];
pub const FAULT_CRATE: f64 = -0.8;

/// The meteor: fire and stone, as the list's Meteor (#43).
pub fn meteor() -> SpellDesign {
    d(&["fire", "stone"], CastType::Rain).named("Meteor")
}

/// The swarm: five plain stones (#59).
pub fn swarm() -> SpellDesign {
    d(&["stone"], CastType::Rain)
        .with(Modifier::Split { count: 5 })
        .named("Meteor swarm")
}

/// The fault line (#42), and with fire, lava in the crack.
pub fn fault(fire: bool) -> SpellDesign {
    if fire {
        d(&["fire", "stone"], CastType::Wave).named("Fire fault")
    } else {
        d(&["stone"], CastType::Wave).named("Fault line")
    }
}

/// The program's lanes.
pub(super) fn lanes() -> Vec<Lane> {
    let cast = |at: f32, design: SpellDesign, aim: AimAt| Cast {
        at: at + START,
        design: Some(design),
        aim,
    };
    let of = |mastery: Mastery, level: f32, casts: Vec<Cast>| Lane {
        mastery,
        level,
        casts,
        ..lane(SpellDesign::default())
    };
    let idle = || of(Mastery::Ascended1, 62.0, Vec::new());
    vec![
        idle(),
        idle(),
        of(
            Mastery::Adept,
            34.0,
            vec![
                cast(3.4, fault(false), AimAt::Lane(-11.0)),
                cast(9.2, fault(true), AimAt::Lane(-11.0)),
            ],
        ),
        of(
            Mastery::Expert,
            48.0,
            vec![cast(0.2, meteor(), AimAt::Focus)],
        ),
        of(
            Mastery::Master,
            66.0,
            vec![cast(1.6, swarm(), AimAt::Focus)],
        ),
        idle(),
    ]
}

/// The program's props: lane 3's loose dummy (its focus) among two crates, two barrels and a
/// crate on a crate; lane 2's two loose dummies and a crate on the fault's line; two loose
/// dummies and a barrel by lane 4's far dummy for the swarm.
pub fn spawn_props(commands: &mut Commands, origin: DVec3) {
    let at = |x: f64, y: f64, z: f64| origin + DVec3::new(x, FLOOR + y, z);
    let x3 = LANES[3];
    spawn_dummy(commands, at(x3, 0.0, METEOR_Z), 3, true, true);
    for (dx, dz, barrel) in [
        (-1.5, 0.9, false),
        (1.6, -0.7, false),
        (1.4, 1.2, true),
        (-1.3, -1.4, true),
    ] {
        let y = if barrel { 0.45 } else { 0.38 };
        spawn_crate(commands, at(x3 + dx, y, METEOR_Z + dz), 3, false, barrel);
    }
    spawn_crate(
        commands,
        at(x3 - 1.5, 1.15, METEOR_Z + 0.9),
        3,
        false,
        false,
    );
    let x2 = LANES[2];
    for z in FAULT_DUMMIES {
        spawn_dummy(commands, at(x2, 0.0, z), 2, false, true);
    }
    spawn_crate(commands, at(x2 + 0.3, 0.38, FAULT_CRATE), 2, false, false);
    let x4 = LANES[4];
    spawn_dummy(commands, at(x4 - 1.8, 0.0, DUMMY_Z + 1.6), 4, false, true);
    spawn_dummy(commands, at(x4 + 1.9, 0.0, DUMMY_Z - 0.8), 4, false, true);
    spawn_crate(commands, at(x4 + 0.6, 0.45, DUMMY_Z + 2.4), 4, false, true);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::{CastContext, compile};
    use crate::schools::Power;

    #[test]
    fn the_spells_are_a_meteor_a_swarm_and_fault_lines() {
        let ctx = |m: Mastery, level: f32| CastContext::new(m, Power::at_level(level));
        let m = compile(&meteor(), ctx(Mastery::Expert, 48.0)).unwrap();
        assert!(m.payload.meteor_m > 0.8, "{}", m.payload.meteor_m);
        assert_eq!(m.carrier.count, 1);
        let s = compile(&swarm(), ctx(Mastery::Master, 66.0)).unwrap();
        assert_eq!(s.carrier.count, 5);
        assert!(s.payload.meteor_m > 0.8);
        for fire in [false, true] {
            let f = compile(&fault(fire), ctx(Mastery::Adept, 34.0)).unwrap();
            assert!(f.payload.fault && f.payload.meteor_m == 0.0);
        }
        // The swarm lands round the far dummies, not on the dais' props.
        assert!(s.carrier.radius < 4.0, "{}", s.carrier.radius);
    }
}
